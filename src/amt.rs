use crate::error::{Error, Result};
use crate::heci::HeciContext;

// PTHI version
const PTHI_MAJOR_VERSION: u8 = 1;
const PTHI_MINOR_VERSION: u8 = 1;

// PTHI header sizes
const PTHI_HEADER_SIZE: usize = 12;
const PTHI_RESP_HEADER_SIZE: usize = 16;

// Command IDs
const PTHI_GET_CODE_VERSIONS_REQUEST: u32 = 0x0400001A;
const PTHI_GET_PROVISIONING_STATE_REQUEST: u32 = 0x04000011;
const PTHI_SET_DNS_SUFFIX_REQUEST: u32 = 0x0400002F;
const PTHI_GET_DNS_SUFFIX_REQUEST: u32 = 0x04000036;
const PTHI_GET_LAN_INTERFACE_SETTINGS_REQUEST: u32 = 0x04000048;
const PTHI_GET_UUID_REQUEST: u32 = 0x0400005C;
const PTHI_GET_LOCAL_SYSTEM_ACCOUNT_REQUEST: u32 = 0x04000067;
const PTHI_GET_CONTROL_MODE_REQUEST: u32 = 0x0400006B;
const PTHI_UNPROVISION_REQUEST: u32 = 0x04000010;

pub const MAX_DNS_SUFFIX_LEN: usize = 192;

// AMT status codes
const AMT_STATUS_SUCCESS: u32 = 0;

// Control mode values
pub const AMT_CONTROL_MODE_PRE_PROVISIONING: u32 = 0;
pub const AMT_CONTROL_MODE_CCM: u32 = 1;
pub const AMT_CONTROL_MODE_ACM: u32 = 2;

// LSA field lengths
pub const LSA_USERNAME_LEN: usize = 33;
pub const LSA_PASSWORD_LEN: usize = 33;

pub struct LsaCredentials {
    pub username: [u8; LSA_USERNAME_LEN],
    pub password: [u8; LSA_PASSWORD_LEN],
}

/// Build a PTHI request header (12 bytes) into `buf` at offset 0.
fn make_header(buf: &mut [u8], command: u32, payload_length: u32) {
    buf[0] = PTHI_MAJOR_VERSION;
    buf[1] = PTHI_MINOR_VERSION;
    buf[2] = 0; // reserved
    buf[3] = 0;
    buf[4..8].copy_from_slice(&command.to_le_bytes());
    buf[8..12].copy_from_slice(&payload_length.to_le_bytes());
}

/// Send a PTHI command and receive the response. Checks PTHI status.
fn pthi_call(heci: &mut HeciContext, cmd: &[u8], resp: &mut [u8]) -> Result<u32> {
    let command = u32::from_le_bytes([cmd[4], cmd[5], cmd[6], cmd[7]]);
    dprintln!();
    dprintln!("--- PTHI Call: cmd=0x{:08x} len={} ---", command, cmd.len());

    // Brief delay to let ME settle
    #[cfg(feature = "uefi-target")]
    uefi::boot::stall(10_000);
    #[cfg(any(feature = "windows-target", feature = "linux-target"))]
    std::thread::sleep(std::time::Duration::from_millis(10));

    heci.send(cmd)?;

    let received = heci.receive(resp)?;

    dprintln!("PTHI: Got {} bytes back", received);

    if (received as usize) < PTHI_RESP_HEADER_SIZE {
        dprintln!("PTHI: Short response ({} bytes, need {})", received, PTHI_RESP_HEADER_SIZE);
        return Err(Error::DeviceError);
    }

    let status = u32::from_le_bytes([resp[12], resp[13], resp[14], resp[15]]);
    let resp_cmd = u32::from_le_bytes([resp[4], resp[5], resp[6], resp[7]]);
    dprintln!("PTHI: Response cmd=0x{:08x} status={}", resp_cmd, status);
    if status != AMT_STATUS_SUCCESS {
        dprintln!("PTHI: Command 0x{:08x} returned AMT error {}", resp_cmd, status);
        return Err(Error::AmtStatus(status));
    }

    dprintln!("--- PTHI Call OK ---");

    Ok(received)
}

pub fn get_control_mode(heci: &mut HeciContext) -> Result<u32> {
    dprintln!("AMT: GetControlMode");
    let mut req = [0u8; PTHI_HEADER_SIZE];
    make_header(&mut req, PTHI_GET_CONTROL_MODE_REQUEST, 0);

    let mut resp = [0u8; 20]; // 16 header + 4 state
    pthi_call(heci, &req, &mut resp)?;

    let state = u32::from_le_bytes([resp[16], resp[17], resp[18], resp[19]]);
    dprintln!("AMT: ControlMode raw value = {}", state);
    Ok(state)
}

pub fn get_provisioning_state(heci: &mut HeciContext) -> Result<u32> {
    dprintln!("AMT: GetProvisioningState");
    let mut req = [0u8; PTHI_HEADER_SIZE];
    make_header(&mut req, PTHI_GET_PROVISIONING_STATE_REQUEST, 0);

    let mut resp = [0u8; 20];
    pthi_call(heci, &req, &mut resp)?;

    let state = u32::from_le_bytes([resp[16], resp[17], resp[18], resp[19]]);
    dprintln!("AMT: ProvisioningState raw value = {}", state);
    Ok(state)
}

pub fn get_uuid(heci: &mut HeciContext) -> Result<[u8; 16]> {
    dprintln!("AMT: GetUUID");
    let mut req = [0u8; PTHI_HEADER_SIZE];
    make_header(&mut req, PTHI_GET_UUID_REQUEST, 0);

    let mut resp = [0u8; 32]; // 16 header + 16 uuid
    pthi_call(heci, &req, &mut resp)?;

    let mut uuid = [0u8; 16];
    uuid.copy_from_slice(&resp[16..32]);
    Ok(uuid)
}

pub fn get_local_system_account(heci: &mut HeciContext) -> Result<LsaCredentials> {
    dprintln!("AMT: GetLocalSystemAccount");

    // Build request: header(length=40) + 40 reserved bytes = 52 bytes
    let mut req = [0u8; 52];
    make_header(&mut req, PTHI_GET_LOCAL_SYSTEM_ACCOUNT_REQUEST, 40);

    dprintln!("AMT: LSA request size=52 (header.length=40)");

    let mut resp_buf = [0u8; 256];
    let resp_len = pthi_call(heci, &req, &mut resp_buf)?;

    dprintln!("AMT: LSA response total={} bytes", resp_len);

    let min_len = PTHI_RESP_HEADER_SIZE + LSA_USERNAME_LEN + LSA_PASSWORD_LEN;
    if (resp_len as usize) < min_len {
        dprintln!("AMT: LSA response too short ({} bytes)", resp_len);
        return Err(Error::DeviceError);
    }

    let data = &resp_buf[PTHI_RESP_HEADER_SIZE..];

    let mut creds = LsaCredentials {
        username: [0u8; LSA_USERNAME_LEN],
        password: [0u8; LSA_PASSWORD_LEN],
    };

    creds.username.copy_from_slice(&data[..LSA_USERNAME_LEN]);
    creds.username[LSA_USERNAME_LEN - 1] = 0;

    creds.password.copy_from_slice(&data[LSA_USERNAME_LEN..LSA_USERNAME_LEN + LSA_PASSWORD_LEN]);
    creds.password[LSA_PASSWORD_LEN - 1] = 0;

    let pwd_len = creds.password.iter().position(|&b| b == 0).unwrap_or(LSA_PASSWORD_LEN);
    let username = core::str::from_utf8(&creds.username[..creds.username.iter().position(|&b| b == 0).unwrap_or(0)]).unwrap_or("?");
    dprintln!("AMT: LSA username='{}' password_len={}", username, pwd_len);

    Ok(creds)
}

/// Result from GetCodeVersions — contains FW version, build number, SKU, etc.
pub struct CodeVersionEntry {
    pub description: [u8; 20],
    pub desc_len: usize,
    pub version: [u8; 20],
    pub ver_len: usize,
}

pub struct CodeVersions {
    pub bios_version: [u8; 65],
    pub entries: [CodeVersionEntry; 20],
    pub count: usize,
}

impl CodeVersions {
    pub fn find(&self, key: &[u8]) -> Option<&[u8]> {
        for i in 0..self.count {
            let e = &self.entries[i];
            if e.desc_len == key.len() && e.description[..e.desc_len] == *key {
                return Some(&e.version[..e.ver_len]);
            }
        }
        None
    }
}

pub fn get_code_versions(heci: &mut HeciContext) -> Result<CodeVersions> {
    dprintln!("AMT: GetCodeVersions");
    let mut req = [0u8; PTHI_HEADER_SIZE];
    make_header(&mut req, PTHI_GET_CODE_VERSIONS_REQUEST, 0);

    let mut resp = [0u8; 4096];
    let resp_len = pthi_call(heci, &req, &mut resp)?;

    let data = &resp[PTHI_RESP_HEADER_SIZE..resp_len as usize];

    let mut cv = CodeVersions {
        bios_version: [0; 65],
        entries: core::array::from_fn(|_| CodeVersionEntry {
            description: [0; 20],
            desc_len: 0,
            version: [0; 20],
            ver_len: 0,
        }),
        count: 0,
    };

    // BIOS version: 65 bytes
    if data.len() < 65 {
        return Ok(cv);
    }
    let copy_len = 65.min(data.len());
    cv.bios_version[..copy_len].copy_from_slice(&data[..copy_len]);

    // Versions count (u32 LE) at offset 65
    if data.len() < 69 {
        return Ok(cv);
    }
    let versions_count = u32::from_le_bytes([data[65], data[66], data[67], data[68]]) as usize;
    cv.count = versions_count.min(20);

    // Each AMTVersionType = 2x AMTUnicodeString = 2x (u16 length + [20]u8 string) = 44 bytes
    let mut offset = 69;
    for i in 0..cv.count {
        if offset + 44 > data.len() {
            cv.count = i;
            break;
        }
        let desc_len = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        let dl = desc_len.min(20);
        cv.entries[i].description[..dl].copy_from_slice(&data[offset..offset + dl]);
        cv.entries[i].desc_len = dl;
        offset += 20;

        let ver_len = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2;
        let vl = ver_len.min(20);
        cv.entries[i].version[..vl].copy_from_slice(&data[offset..offset + vl]);
        cv.entries[i].ver_len = vl;
        offset += 20;
    }

    Ok(cv)
}

pub fn set_dns_suffix(heci: &mut HeciContext, suffix: &[u8]) -> Result<()> {
    dprintln!("AMT: SetPKIDNSSuffix len={}", suffix.len());

    if suffix.len() > MAX_DNS_SUFFIX_LEN {
        return Err(Error::BufferTooSmall);
    }

    // Request: header(12) + u16 length + N bytes suffix data
    let payload_len = 2 + suffix.len();
    let total_len = PTHI_HEADER_SIZE + payload_len;

    let mut req = [0u8; PTHI_HEADER_SIZE + 2 + MAX_DNS_SUFFIX_LEN];
    make_header(&mut req, PTHI_SET_DNS_SUFFIX_REQUEST, payload_len as u32);

    let len_le = (suffix.len() as u16).to_le_bytes();
    req[PTHI_HEADER_SIZE..PTHI_HEADER_SIZE + 2].copy_from_slice(&len_le);
    req[PTHI_HEADER_SIZE + 2..PTHI_HEADER_SIZE + 2 + suffix.len()].copy_from_slice(suffix);

    let mut resp = [0u8; 32];
    pthi_call(heci, &req[..total_len], &mut resp)?;

    Ok(())
}

pub fn get_dns_suffix(heci: &mut HeciContext) -> Result<([u8; 256], usize)> {
    dprintln!("AMT: GetDNSSuffix");
    let mut req = [0u8; PTHI_HEADER_SIZE];
    make_header(&mut req, PTHI_GET_DNS_SUFFIX_REQUEST, 0);

    let mut resp = [0u8; 1100];
    let resp_len = pthi_call(heci, &req, &mut resp)?;

    let data = &resp[PTHI_RESP_HEADER_SIZE..resp_len as usize];
    let mut suffix = [0u8; 256];

    if data.len() < 2 {
        return Ok((suffix, 0));
    }

    let str_len = u16::from_le_bytes([data[0], data[1]]) as usize;
    let copy_len = str_len.min(255).min(data.len() - 2);
    suffix[..copy_len].copy_from_slice(&data[2..2 + copy_len]);
    Ok((suffix, copy_len))
}

pub struct LanInterfaceSettings {
    pub ipv4_addr: [u8; 4],
    pub dhcp_enabled: bool,
    pub dhcp_mode: u8,
    pub link_status: u8,
    pub mac_addr: [u8; 6],
}

pub fn get_lan_interface_settings(heci: &mut HeciContext, wireless: bool) -> Result<LanInterfaceSettings> {
    dprintln!("AMT: GetLANInterfaceSettings(wireless={})", wireless);
    let mut req = [0u8; 16]; // 12 header + 4 interface index
    make_header(&mut req, PTHI_GET_LAN_INTERFACE_SETTINGS_REQUEST, 4);
    let idx: u32 = if wireless { 1 } else { 0 };
    req[12..16].copy_from_slice(&idx.to_le_bytes());

    let mut resp = [0u8; 64];
    let resp_len = pthi_call(heci, &req, &mut resp)?;

    let data = &resp[PTHI_RESP_HEADER_SIZE..resp_len as usize];
    if data.len() < 12 {
        return Err(Error::DeviceError);
    }

    let ip = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let dhcp = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);

    let mut settings = LanInterfaceSettings {
        ipv4_addr: [(ip >> 24) as u8, (ip >> 16) as u8, (ip >> 8) as u8, ip as u8],
        dhcp_enabled: dhcp == 1,
        dhcp_mode: 0,
        link_status: 0,
        mac_addr: [0; 6],
    };

    if data.len() > 12 { settings.dhcp_mode = data[12]; }
    if data.len() > 13 { settings.link_status = data[13]; }
    if data.len() >= 20 {
        settings.mac_addr.copy_from_slice(&data[14..20]);
    }

    Ok(settings)
}

pub fn unprovision(heci: &mut HeciContext) -> Result<()> {
    dprintln!("AMT: Unprovision");

    // Build request: header(length=4) + mode(u32=0) = 16 bytes
    let mut req = [0u8; 16];
    make_header(&mut req, PTHI_UNPROVISION_REQUEST, 4);
    // mode = 0 (full unprovision) — already zeroed

    let mut resp_buf = [0u8; 64];
    let resp_len = pthi_call(heci, &req, &mut resp_buf)?;

    if resp_len as usize >= PTHI_RESP_HEADER_SIZE + 4 {
        let state = u32::from_le_bytes([
            resp_buf[16], resp_buf[17], resp_buf[18], resp_buf[19],
        ]);
        dprintln!("AMT: Unprovision result state={}", state);
    }

    Ok(())
}

pub fn control_mode_str(mode: u32) -> &'static str {
    match mode {
        AMT_CONTROL_MODE_PRE_PROVISIONING => "Pre-Provisioning",
        AMT_CONTROL_MODE_CCM => "Client Control Mode (CCM)",
        AMT_CONTROL_MODE_ACM => "Admin Control Mode (ACM)",
        _ => "Unknown",
    }
}
