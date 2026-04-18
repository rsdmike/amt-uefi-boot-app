use crate::error::{Error, Result};
use crate::heci::transport::{AppHeciHooks, AppHeciTransport};
use wsman_apf::session::ApfSession;

type Session = ApfSession<AppHeciTransport, AppHeciHooks>;

use crate::md5;
use crate::str_util::*;

pub const HTTP_MAX_BODY: usize = 4096;

pub struct HttpResponse {
    pub status_code: u32,
    pub body: [u8; HTTP_MAX_BODY],
    pub body_len: u32,
    pub auth_realm: [u8; 128],
    pub auth_nonce: [u8; 128],
    pub auth_qop: [u8; 32],
    pub auth_opaque: [u8; 128],
}

pub struct DigestAuth {
    pub username: [u8; 64],
    pub password: [u8; 64],
    pub realm: [u8; 128],
    pub nonce: [u8; 128],
    pub qop: [u8; 32],
    pub opaque: [u8; 128],
    pub nc: u32,
}

fn copy_str(dst: &mut [u8], src: &[u8]) {
    let len = ascii_len(src).min(dst.len() - 1);
    dst[..len].copy_from_slice(&src[..len]);
    dst[len] = 0;
}

/// Parse HTTP response from raw bytes.
fn parse_response(data: &[u8], len: u32, resp: &mut HttpResponse) -> Result<()> {
    resp.status_code = 0;
    resp.body_len = 0;
    resp.body[0] = 0;
    resp.auth_realm[0] = 0;
    resp.auth_nonce[0] = 0;
    resp.auth_qop[0] = 0;
    resp.auth_opaque[0] = 0;

    let raw = &data[..len as usize];

    // Find "HTTP/1.1 " or "HTTP/1.0 "
    let sp = ascii_find(raw, b"HTTP/1.1 ")
        .or_else(|| ascii_find(raw, b"HTTP/1.0 "));
    let sp = match sp {
        Some(pos) => pos + 9,
        None => return Err(Error::ProtocolError),
    };

    // Parse 3-digit status code
    resp.status_code = 0;
    for i in 0..3 {
        if sp + i < raw.len() && raw[sp + i] >= b'0' && raw[sp + i] <= b'9' {
            resp.status_code = resp.status_code * 10 + (raw[sp + i] - b'0') as u32;
        }
    }

    // Find body start (after \r\n\r\n)
    let body_start = ascii_find(raw, b"\r\n\r\n")
        .map(|pos| pos + 4)
        .unwrap_or(raw.len());

    // Extract Content-Length
    let mut content_length: u32 = 0;
    if let Some(cl_pos) = ascii_find(raw, b"Content-Length:")
        .or_else(|| ascii_find(raw, b"content-length:"))
    {
        let mut p = cl_pos + 15;
        while p < raw.len() && raw[p] == b' ' { p += 1; }
        while p < raw.len() && raw[p] >= b'0' && raw[p] <= b'9' {
            content_length = content_length * 10 + (raw[p] - b'0') as u32;
            p += 1;
        }
    }

    // Extract digest auth from 401
    if resp.status_code == 401 {
        if let Some(auth_pos) = ascii_find(raw, b"WWW-Authenticate:")
            .or_else(|| ascii_find(raw, b"www-authenticate:"))
        {
            let auth_line = &raw[auth_pos..];
            extract_field(auth_line, b"realm", &mut resp.auth_realm);
            extract_field(auth_line, b"nonce", &mut resp.auth_nonce);
            extract_field(auth_line, b"qop", &mut resp.auth_qop);
            extract_field(auth_line, b"opaque", &mut resp.auth_opaque);
        }
    }

    // Check for chunked transfer encoding
    let is_chunked = ascii_find(&raw[..body_start], b"Transfer-Encoding: chunked").is_some()
        || ascii_find(&raw[..body_start], b"transfer-encoding: chunked").is_some()
        || ascii_find(&raw[..body_start], b"Transfer-Encoding:chunked").is_some();

    if is_chunked {
        // Decode chunked body: each chunk is "<hex-size>\r\n<data>\r\n", ending with "0\r\n"
        let mut src = body_start;
        let mut dst: usize = 0;

        while src < raw.len() && dst < HTTP_MAX_BODY - 1 {
            // Parse hex chunk size
            let mut chunk_size: usize = 0;
            let mut has_digits = false;
            while src < raw.len() {
                let b = raw[src];
                let digit = match b {
                    b'0'..=b'9' => Some((b - b'0') as usize),
                    b'a'..=b'f' => Some((b - b'a' + 10) as usize),
                    b'A'..=b'F' => Some((b - b'A' + 10) as usize),
                    _ => None,
                };
                match digit {
                    Some(d) => { chunk_size = chunk_size * 16 + d; has_digits = true; src += 1; }
                    None => break,
                }
            }

            if !has_digits || chunk_size == 0 {
                break; // Final chunk or parse error
            }

            // Skip \r\n after chunk size
            if src + 1 < raw.len() && raw[src] == b'\r' && raw[src + 1] == b'\n' {
                src += 2;
            }

            // Copy chunk data
            let copy_len = chunk_size.min(raw.len() - src).min(HTTP_MAX_BODY - 1 - dst);
            resp.body[dst..dst + copy_len].copy_from_slice(&raw[src..src + copy_len]);
            dst += copy_len;
            src += chunk_size;

            // Skip trailing \r\n after chunk data
            if src + 1 < raw.len() && raw[src] == b'\r' && raw[src + 1] == b'\n' {
                src += 2;
            }
        }

        resp.body[dst] = 0;
        resp.body_len = dst as u32;
        dprintln!("HTTP: Decoded chunked body: {} bytes", dst);
    } else {
        // Copy body using Content-Length
        let avail = raw.len() - body_start;
        let mut to_copy = avail;
        if content_length > 0 && (content_length as usize) < to_copy {
            to_copy = content_length as usize;
        }
        if to_copy > HTTP_MAX_BODY - 1 {
            to_copy = HTTP_MAX_BODY - 1;
        }

        resp.body[..to_copy].copy_from_slice(&raw[body_start..body_start + to_copy]);
        resp.body[to_copy] = 0;
        resp.body_len = to_copy as u32;
    }

    Ok(())
}

/// Build Digest Authorization header value into buf. Returns length.
fn build_auth_header(auth: &mut DigestAuth, buf: &mut [u8]) -> usize {
    let mut digest = [0u8; 16];
    let mut ha1_hex = [0u8; 33];
    let mut ha2_hex = [0u8; 33];
    let mut resp_hex = [0u8; 33];
    let mut nc_str = [0u8; 9];
    let cnonce = b"uefiboot";

    // HA1 = MD5(username:realm:password)
    {
        let mut a1 = [0u8; 256];
        let mut p = 0;
        p = ascii_append(&mut a1, p, &auth.username);
        a1[p] = b':'; p += 1;
        p = ascii_append(&mut a1, p, &auth.realm);
        a1[p] = b':'; p += 1;
        p = ascii_append(&mut a1, p, &auth.password);
        md5::md5_hash(&a1[..p], &mut digest);
        md5::md5_hex(&digest, &mut ha1_hex);
    }

    // HA2 = MD5("POST:/wsman")
    {
        md5::md5_hash(b"POST:/wsman", &mut digest);
        md5::md5_hex(&digest, &mut ha2_hex);
    }

    // Nonce count
    auth.nc += 1;
    append_hex32(&mut nc_str, 0, auth.nc);
    nc_str[8] = 0;

    // response = MD5(HA1:nonce:nc:cnonce:qop:HA2) or MD5(HA1:nonce:HA2)
    {
        let mut resp_input = [0u8; 512];
        let mut p = 0;
        p = ascii_append(&mut resp_input, p, &ha1_hex);
        resp_input[p] = b':'; p += 1;
        p = ascii_append(&mut resp_input, p, &auth.nonce);
        resp_input[p] = b':'; p += 1;

        if auth.qop[0] != 0 {
            p = append_bytes(&mut resp_input, p, &nc_str[..8]);
            resp_input[p] = b':'; p += 1;
            p = append_bytes(&mut resp_input, p, cnonce);
            resp_input[p] = b':'; p += 1;
            p = append_bytes(&mut resp_input, p, b"auth");
            resp_input[p] = b':'; p += 1;
        }

        p = ascii_append(&mut resp_input, p, &ha2_hex);
        md5::md5_hash(&resp_input[..p], &mut digest);
        md5::md5_hex(&digest, &mut resp_hex);
    }

    // Build: Digest username="...",realm="...",... (AMT requires quoted qop/nc/cnonce)
    let mut o = 0;
    o = append_bytes(buf, o, b"Digest username=\"");
    o = ascii_append(buf, o, &auth.username);
    o = append_bytes(buf, o, b"\",realm=\"");
    o = ascii_append(buf, o, &auth.realm);
    o = append_bytes(buf, o, b"\",nonce=\"");
    o = ascii_append(buf, o, &auth.nonce);
    o = append_bytes(buf, o, b"\",uri=\"/wsman\",response=\"");
    o = ascii_append(buf, o, &resp_hex);
    o = append_bytes(buf, o, b"\"");

    if auth.opaque[0] != 0 {
        o = append_bytes(buf, o, b",opaque=\"");
        o = ascii_append(buf, o, &auth.opaque);
        o = append_bytes(buf, o, b"\"");
    }

    if auth.qop[0] != 0 {
        o = append_bytes(buf, o, b",qop=\"auth\",nc=\"");
        o = append_bytes(buf, o, &nc_str[..8]);
        o = append_bytes(buf, o, b"\",cnonce=\"");
        o = append_bytes(buf, o, cnonce);
        o = append_bytes(buf, o, b"\"");
    }

    if o < buf.len() {
        buf[o] = 0;
    }
    o
}

/// POST body to /wsman over LME APF channel.
pub fn post_wsman(lme: &mut Session, body: &[u8], auth: Option<&mut DigestAuth>, resp: &mut HttpResponse) -> Result<()> {
    use core::sync::atomic::{AtomicBool, Ordering};
    static IN_USE: AtomicBool = AtomicBool::new(false);
    if IN_USE.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        dprintln!("HTTP: post_wsman reentered — aborting");
        return Err(Error::Aborted);
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) { IN_USE.store(false, Ordering::Release); }
    }
    let _guard = Guard;

    static mut REQ_BUF: [u8; 2048] = [0; 2048];
    // SAFETY: IN_USE guard above guarantees no concurrent/reentrant access
    let req = unsafe { &mut *(&raw mut REQ_BUF) };
    let mut pos = 0;

    // Request line + headers. Use Connection: close so AMT closes the APF channel
    // after the response — otherwise keep-alive leaves the channel open and we hang
    // waiting for CHANNEL_CLOSE that never comes.
    pos = append_bytes(req, pos, b"POST /wsman HTTP/1.1\r\n");
    pos = append_bytes(req, pos, b"Host: 127.0.0.1:16992\r\n");
    pos = append_bytes(req, pos, b"Connection: close\r\n");
    pos = append_bytes(req, pos, b"Content-Type: application/soap+xml; charset=utf-8\r\n");

    // Content-Length
    pos = append_bytes(req, pos, b"Content-Length: ");
    pos = append_u32(req, pos, body.len() as u32);
    pos = append_bytes(req, pos, b"\r\n");

    // Authorization
    if let Some(auth) = auth {
        if auth.nonce[0] != 0 {
            pos = append_bytes(req, pos, b"Authorization: ");
            let mut auth_val = [0u8; 512];
            let auth_len = build_auth_header(auth, &mut auth_val);
            pos = append_bytes(req, pos, &auth_val[..auth_len]);
            pos = append_bytes(req, pos, b"\r\n");
        }
    }

    // End of headers
    pos = append_bytes(req, pos, b"\r\n");

    // Body
    pos = append_bytes(req, pos, body);

    dprintln!("HTTP: Sending {} bytes ({} header + {} body)", pos, pos - body.len(), body.len());

    // Re-open APF channel if not currently active.
    // NOTE: use `reopen_channel` (not `channel_open`) — on UEFI it first does a
    // full LME session reconnect, which is required because some ME firmware
    // sends HBM_CLIENT_DISCONNECT_REQ on a plain reopen over the same session
    // and the platform hard-resets during our handling of that disconnect.
    if !lme.channel_active() {
        dprintln!("HTTP: Reopening APF channel (full LME reconnect)...");
        lme.reopen_channel().map_err(|_| Error::DeviceError)?;
    }

    // Send through LME APF channel
    lme.send_bytes(&req[..pos]).map_err(|_| Error::DeviceError)?;

    // Receive response — caller-owned buffer.
    let mut rx_buf = [0u8; 4096];
    let rx_len = lme.recv_bytes(&mut rx_buf)
        .map_err(|_| Error::DeviceError)? as u32;

    dprintln!("HTTP: Received {} bytes", rx_len);

    // Parse response
    parse_response(&rx_buf, rx_len, resp)?;

    dprintln!("HTTP: Status {}, body {} bytes", resp.status_code, resp.body_len);

    Ok(())
}

/// Populate DigestAuth from a 401 challenge response.
pub fn digest_from_challenge(auth: &mut DigestAuth, challenge: &HttpResponse, username: &[u8], password: &[u8]) {
    copy_str(&mut auth.username, username);
    copy_str(&mut auth.password, password);
    copy_str(&mut auth.realm, &challenge.auth_realm);
    copy_str(&mut auth.nonce, &challenge.auth_nonce);
    copy_str(&mut auth.qop, &challenge.auth_qop);
    copy_str(&mut auth.opaque, &challenge.auth_opaque);
    auth.nc = 0;

    let realm_str = core::str::from_utf8(&auth.realm[..ascii_len(&auth.realm)]).unwrap_or("?");
    let nonce_str = core::str::from_utf8(&auth.nonce[..ascii_len(&auth.nonce)]).unwrap_or("?");
    let user_str = core::str::from_utf8(&auth.username[..ascii_len(&auth.username)]).unwrap_or("?");
    dprintln!("HTTP: Digest auth configured:");
    dprintln!("  user='{}' realm='{}'", user_str, realm_str);
    dprintln!("  nonce='{}'", nonce_str);
}
