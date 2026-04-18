use crate::error::{Error, Result};
use crate::heci::transport::{AppHeciHooks, AppHeciTransport};
use wsman_apf::session::ApfSession;
use crate::amt::LsaCredentials;
use crate::http::{self, HttpResponse, DigestAuth};
use crate::md5;
use crate::str_util::*;

type Session = ApfSession<AppHeciTransport, AppHeciHooks>;

pub struct WsmanCcmResult {
    pub setup_return: u32,
    pub digest_realm: [u8; 128],
}

const SOAP_ENV_OPEN: &[u8] = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<Envelope xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"\
 xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\"\
 xmlns:a=\"http://schemas.xmlsoap.org/ws/2004/08/addressing\"\
 xmlns:w=\"http://schemas.dmtf.org/wbem/wsman/1/wsman.xsd\"\
 xmlns=\"http://www.w3.org/2003/05/soap-envelope\">";

const SOAP_ENV_CLOSE: &[u8] = b"</Envelope>";

/// Extract value between <tag>value</tag> where tag name ends with `suffix`.
fn xml_extract(xml: &[u8], suffix: &[u8], out: &mut [u8]) -> bool {
    let xml_len = ascii_len(xml);
    let suffix_len = suffix.len();
    let mut i = 0;

    while i < xml_len {
        // Find opening '<'
        if xml[i] == b'<' && i + 1 < xml_len && xml[i + 1] != b'/' {
            let tag_start = i + 1;
            let mut tag_end = tag_start;
            while tag_end < xml_len && xml[tag_end] != b'>' && xml[tag_end] != b' ' {
                tag_end += 1;
            }
            if tag_end >= xml_len {
                break;
            }

            let tag_len = tag_end - tag_start;
            if tag_len >= suffix_len {
                let tag_suffix_start = tag_end - suffix_len;
                if xml[tag_suffix_start..tag_end] == suffix[..suffix_len] {
                    // Skip past '>' (skip attributes)
                    while tag_end < xml_len && xml[tag_end] != b'>' {
                        tag_end += 1;
                    }
                    if tag_end >= xml_len {
                        break;
                    }
                    let val_start = tag_end + 1;

                    // Find closing '</'
                    let mut val_end = val_start;
                    while val_end < xml_len && xml[val_end] != b'<' {
                        val_end += 1;
                    }

                    let val_len = (val_end - val_start).min(out.len() - 1);
                    out[..val_len].copy_from_slice(&xml[val_start..val_start + val_len]);
                    out[val_len] = 0;
                    return true;
                }
            }
        }
        i += 1;
    }

    out[0] = 0;
    false
}

fn build_get_general_settings(buf: &mut [u8]) -> usize {
    let mut p = 0;
    p = append_bytes(buf, p, SOAP_ENV_OPEN);
    p = append_bytes(buf, p, b"<Header>");
    p = append_bytes(buf, p, b"<a:Action>http://schemas.xmlsoap.org/ws/2004/09/transfer/Get</a:Action>");
    p = append_bytes(buf, p, b"<a:To>/wsman</a:To>");
    p = append_bytes(buf, p, b"<w:ResourceURI>http://intel.com/wbem/wscim/1/amt-schema/1/AMT_GeneralSettings</w:ResourceURI>");
    p = append_bytes(buf, p, b"<a:MessageID>1</a:MessageID>");
    p = append_bytes(buf, p, b"<a:ReplyTo><a:Address>http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous</a:Address></a:ReplyTo>");
    p = append_bytes(buf, p, b"<w:OperationTimeout>PT60S</w:OperationTimeout>");
    p = append_bytes(buf, p, b"</Header>");
    p = append_bytes(buf, p, b"<Body></Body>");
    p = append_bytes(buf, p, SOAP_ENV_CLOSE);
    if p < buf.len() { buf[p] = 0; }
    p
}

fn build_setup_ccm(buf: &mut [u8], password_hash: &[u8]) -> usize {
    let mut p = 0;
    p = append_bytes(buf, p, SOAP_ENV_OPEN);
    p = append_bytes(buf, p, b"<Header>");
    p = append_bytes(buf, p, b"<a:Action>http://intel.com/wbem/wscim/1/ips-schema/1/IPS_HostBasedSetupService/Setup</a:Action>");
    p = append_bytes(buf, p, b"<a:To>/wsman</a:To>");
    p = append_bytes(buf, p, b"<w:ResourceURI>http://intel.com/wbem/wscim/1/ips-schema/1/IPS_HostBasedSetupService</w:ResourceURI>");
    p = append_bytes(buf, p, b"<a:MessageID>2</a:MessageID>");
    p = append_bytes(buf, p, b"<a:ReplyTo><a:Address>http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous</a:Address></a:ReplyTo>");
    p = append_bytes(buf, p, b"<w:OperationTimeout>PT60S</w:OperationTimeout>");
    p = append_bytes(buf, p, b"</Header>");
    p = append_bytes(buf, p, b"<Body>");
    p = append_bytes(buf, p, b"<h:Setup_INPUT xmlns:h=\"http://intel.com/wbem/wscim/1/ips-schema/1/IPS_HostBasedSetupService\">");
    p = append_bytes(buf, p, b"<h:NetAdminPassEncryptionType>2</h:NetAdminPassEncryptionType>");
    p = append_bytes(buf, p, b"<h:NetworkAdminPassword>");
    p = ascii_append(buf, p, password_hash);
    p = append_bytes(buf, p, b"</h:NetworkAdminPassword>");
    p = append_bytes(buf, p, b"</h:Setup_INPUT>");
    p = append_bytes(buf, p, b"</Body>");
    p = append_bytes(buf, p, SOAP_ENV_CLOSE);
    if p < buf.len() { buf[p] = 0; }
    p
}

/// Perform one WSMAN call with digest auth retry (up to 3 attempts).
fn wsman_call(lme: &mut Session, auth: &mut DigestAuth, body: &[u8], resp: &mut HttpResponse, label: &str) -> Result<()> {
    dprintln!("WSMAN: {} (body={} bytes)", label, body.len());

    for attempt in 0..3 {
        http::post_wsman(lme, body, Some(auth), resp)?;

        if resp.status_code != 401 {
            break;
        }

        dprintln!("WSMAN: Got 401 (attempt {}), updating digest auth...", attempt + 1);
        let realm_str = core::str::from_utf8(&resp.auth_realm[..ascii_len(&resp.auth_realm)]).unwrap_or("?");
        dprintln!("WSMAN: realm='{}'", realm_str);

        // Copy username/password before passing to digest_from_challenge
        let mut username = [0u8; 64];
        let mut password = [0u8; 64];
        username.copy_from_slice(&auth.username);
        password.copy_from_slice(&auth.password);
        http::digest_from_challenge(auth, resp, &username, &password);
    }

    if resp.status_code == 401 {
        dprintln!("WSMAN: Authentication failed after retries");
        return Err(Error::AccessDenied);
    }

    if resp.status_code != 200 {
        dprintln!("WSMAN: HTTP error {}", resp.status_code);
        return Err(Error::HttpStatus(resp.status_code));
    }

    Ok(())
}

/// Hash admin password: MD5("admin:" + digest_realm + ":" + password)
fn hash_admin_password(digest_realm: &[u8], password: &[u8], hex_out: &mut [u8; 33]) {
    let mut input = [0u8; 256];
    let mut p = 0;
    p = append_bytes(&mut input, p, b"admin:");
    p = ascii_append(&mut input, p, digest_realm);
    input[p] = b':'; p += 1;
    p = append_bytes(&mut input, p, password);

    let mut digest = [0u8; 16];
    md5::md5_hash(&input[..p], &mut digest);
    md5::md5_hex(&digest, hex_out);
}

/// Full CCM activation: GetGeneralSettings -> HostBasedSetup -> CommitChanges.
pub fn activate_ccm(lme: &mut Session, lsa: &LsaCredentials, admin_password: &[u8]) -> Result<WsmanCcmResult> {
    use core::sync::atomic::{AtomicBool, Ordering};
    static IN_USE: AtomicBool = AtomicBool::new(false);
    if IN_USE.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        dprintln!("WSMAN: activate_ccm reentered — aborting");
        return Err(Error::Aborted);
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) { IN_USE.store(false, Ordering::Release); }
    }
    let _guard = Guard;

    dprintln!("WSMAN: activate_ccm entered");

    // Static buffers — zeroed in .bss, no stack allocation at all.
    static mut SOAP_BUF: [u8; 2048] = [0; 2048];
    static mut RESP: HttpResponse = HttpResponse {
        status_code: 0, body: [0; crate::http::HTTP_MAX_BODY], body_len: 0,
        auth_realm: [0; 128], auth_nonce: [0; 128], auth_qop: [0; 32], auth_opaque: [0; 128],
    };
    static mut AUTH: DigestAuth = DigestAuth {
        username: [0; 64], password: [0; 64], realm: [0; 128],
        nonce: [0; 128], qop: [0; 32], opaque: [0; 128], nc: 0,
    };

    // SAFETY: IN_USE guard above guarantees no concurrent/reentrant access
    let soap_buf = unsafe { &mut *(&raw mut SOAP_BUF) };
    let resp = unsafe { &mut *(&raw mut RESP) };
    resp.status_code = 0; resp.body_len = 0; resp.body[0] = 0;
    resp.auth_realm[0] = 0; resp.auth_nonce[0] = 0; resp.auth_qop[0] = 0; resp.auth_opaque[0] = 0;
    let auth = unsafe { &mut *(&raw mut AUTH) };
    auth.nc = 0; auth.username[0] = 0; auth.password[0] = 0;
    auth.realm[0] = 0; auth.nonce[0] = 0; auth.qop[0] = 0; auth.opaque[0] = 0;

    let mut result = WsmanCcmResult {
        setup_return: 0xFFFF_FFFF,
        digest_realm: [0; 128],
    };

    // Set up auth with LSA credentials (nonce filled on first 401)
    let user_len = ascii_len(&lsa.username).min(63);
    auth.username[..user_len].copy_from_slice(&lsa.username[..user_len]);
    auth.username[user_len] = 0;

    let pass_len = ascii_len(&lsa.password).min(63);
    auth.password[..pass_len].copy_from_slice(&lsa.password[..pass_len]);
    auth.password[pass_len] = 0;

    let user_str = core::str::from_utf8(&auth.username[..user_len]).unwrap_or("?");
    dprintln!();
    dprintln!("WSMAN: === Step 1/2: GetGeneralSettings ===");
    dprintln!("WSMAN: Using LSA user='{}'", user_str);

    let soap_len = build_get_general_settings(soap_buf);
    dprintln!("WSMAN: SOAP built, len={}", soap_len);

    wsman_call(lme, auth, &soap_buf[..soap_len], resp, "GetGeneralSettings")?;

    // Extract DigestRealm from response XML
    let mut digest_realm = [0u8; 128];
    if !xml_extract(&resp.body[..resp.body_len as usize], b"DigestRealm", &mut digest_realm) {
        dprintln!("WSMAN: Could not find DigestRealm in response");
        return Err(Error::NotFound);
    }

    let realm_len = ascii_len(&digest_realm).min(127);
    result.digest_realm[..realm_len].copy_from_slice(&digest_realm[..realm_len]);
    result.digest_realm[realm_len] = 0;

    let realm_str = core::str::from_utf8(&digest_realm[..realm_len]).unwrap_or("?");
    dprintln!("WSMAN: DigestRealm = '{}'", realm_str);

    // Step 2: HostBasedSetupService/Setup
    dprintln!();
    dprintln!("WSMAN: === Step 2/2: HostBasedSetup (CCM) ===");

    let mut password_hash = [0u8; 33];
    hash_admin_password(&digest_realm, admin_password, &mut password_hash);
    let hash_str = core::str::from_utf8(&password_hash[..32]).unwrap_or("?");
    dprintln!("WSMAN: Password hash = {}", hash_str);

    let soap_len = build_setup_ccm(soap_buf, &password_hash);
    wsman_call(lme, auth, &soap_buf[..soap_len], resp, "HostBasedSetup")?;

    // Extract ReturnValue
    let mut ret_val = [0u8; 16];
    if xml_extract(&resp.body[..resp.body_len as usize], b"ReturnValue", &mut ret_val) {
        result.setup_return = 0;
        for i in 0..ascii_len(&ret_val) {
            if ret_val[i] >= b'0' && ret_val[i] <= b'9' {
                result.setup_return = result.setup_return * 10 + (ret_val[i] - b'0') as u32;
            } else {
                break;
            }
        }
        dprintln!("WSMAN: Setup ReturnValue = {}", result.setup_return);
    }

    if result.setup_return != 0 {
        dprintln!("WSMAN: Setup failed with return value {}", result.setup_return);
        return Err(Error::DeviceError);
    }

    dprintln!();
    dprintln!("WSMAN: CCM activation COMPLETE");
    Ok(result)
}
