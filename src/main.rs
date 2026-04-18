#![cfg_attr(feature = "uefi-target", no_main)]
#![cfg_attr(feature = "uefi-target", no_std)]

/// Set to true to enable verbose debug logging from all modules.
pub const DEBUG_LOG: bool = true;

// --- UEFI log buffer: captures dprintln! output for writing to USB ---

#[cfg(feature = "uefi-target")]
const LOG_BUF_SIZE: usize = 64 * 1024; // 64KB ring buffer

#[cfg(feature = "uefi-target")]
static mut LOG_BUF: [u8; LOG_BUF_SIZE] = [0; LOG_BUF_SIZE];

#[cfg(feature = "uefi-target")]
static mut LOG_POS: usize = 0;

#[cfg(feature = "uefi-target")]
pub struct LogWriter;

#[cfg(feature = "uefi-target")]
const LOG_TRUNC_MARKER: &[u8] = b"\n--LOG TRUNCATED--\n";

#[cfg(feature = "uefi-target")]
impl core::fmt::Write for LogWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // Reserve tail space for the truncation marker so it can always be written.
        const TAIL: usize = 32; // enough for LOG_TRUNC_MARKER
        let limit = LOG_BUF_SIZE - TAIL;
        unsafe {
            let bytes = s.as_bytes();
            for &b in bytes {
                if LOG_POS < limit {
                    LOG_BUF[LOG_POS] = b;
                    LOG_POS += 1;
                } else if LOG_POS < LOG_BUF_SIZE {
                    // First overflow: stamp the truncation marker once.
                    for &m in LOG_TRUNC_MARKER {
                        if LOG_POS < LOG_BUF_SIZE {
                            LOG_BUF[LOG_POS] = m;
                            LOG_POS += 1;
                        }
                    }
                    // Further writes are dropped silently.
                    break;
                }
            }
        }
        Ok(())
    }
}

/// Flush the log buffer to \debug.log on the boot volume.
#[cfg(feature = "uefi-target")]
pub fn flush_log() {
    use uefi::proto::media::file::{File, FileMode, FileAttribute, FileType};

    let pos = unsafe { LOG_POS };
    if pos == 0 { return; }

    let image = uefi::boot::image_handle();
    let Ok(mut fs) = uefi::boot::get_image_file_system(image) else { return };
    let Ok(mut root) = fs.open_volume() else { return };

    let filename = uefi::cstr16!("debug.log");
    let Ok(handle) = root.open(filename, FileMode::CreateReadWrite, FileAttribute::empty()) else { return };

    if let Ok(FileType::Regular(mut file)) = handle.into_type() {
        let buf = unsafe { &LOG_BUF[..pos] };
        let _ = file.write(buf);
    }
}

/// Format a timestamp "HH:MM:SS.mmm" for the debug log.
#[cfg(feature = "uefi-target")]
pub fn write_timestamp(out: &mut impl core::fmt::Write) {
    match uefi::runtime::get_time() {
        Ok(t) => {
            let ms = t.nanosecond() / 1_000_000;
            let _ = write!(out, "[{:02}:{:02}:{:02}.{:03}] ", t.hour(), t.minute(), t.second(), ms);
        }
        Err(_) => { let _ = out.write_str("[??:??:??.???] "); }
    }
}

#[cfg(any(feature = "windows-target", feature = "linux-target"))]
pub fn format_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let total_secs = now.as_secs();
    let ms = now.subsec_millis();
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hrs = (total_secs / 3600) % 24;
    format!("[{:02}:{:02}:{:02}.{:03}] ", hrs, mins, secs, ms)
}

/// Debug println — gated by DEBUG_LOG. Every line is timestamped.
/// On UEFI: writes to log buffer only (flushed to debug.log on USB).
/// On Windows/Linux: writes to stdout.
#[macro_export]
macro_rules! dprintln {
    () => { if $crate::DEBUG_LOG {
        #[cfg(feature = "uefi-target")] {
            use core::fmt::Write;
            $crate::write_timestamp(&mut $crate::LogWriter);
            let _ = $crate::LogWriter.write_str("\n");
        }
        #[cfg(any(feature = "windows-target", feature = "linux-target"))] { println!("{}", $crate::format_timestamp()); }
    } };
    ($($arg:tt)*) => { if $crate::DEBUG_LOG {
        #[cfg(feature = "uefi-target")] {
            use core::fmt::Write;
            $crate::write_timestamp(&mut $crate::LogWriter);
            let _ = write!($crate::LogWriter, $($arg)*);
            let _ = $crate::LogWriter.write_str("\n");
        }
        #[cfg(any(feature = "windows-target", feature = "linux-target"))] { println!("{}{}", $crate::format_timestamp(), format!($($arg)*)); }
    } };
}

mod error;
#[cfg(feature = "uefi-target")]
mod pci;
mod heci;
mod amt;
mod md5;
mod str_util;
mod http;
mod wsman;
mod wsman_glue;
mod ui;
#[cfg(feature = "uefi-target")]
mod font;

#[cfg(feature = "uefi-target")]
use uefi::prelude::*;

use crate::heci::HeciContext;
use crate::heci::transport::{AppHeciHooks, AppHeciTransport};
use crate::amt::{AMT_CONTROL_MODE_PRE_PROVISIONING, AMT_CONTROL_MODE_ACM};
use wsman_apf::session::ApfSession;

const DEFAULT_AMT_PASSWORD: &[u8] = b"P@ssw0rd";

fn sleep_ms(ms: u64) {
    #[cfg(feature = "uefi-target")]
    uefi::boot::stall((ms * 1000) as usize);
    #[cfg(any(feature = "windows-target", feature = "linux-target"))]
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

fn do_amt_info(heci: &mut HeciContext) {
    ui::show_working("Loading AMT Information...");

    // After deactivation, AMT may refuse PTHI calls for up to ~60s.
    // Use get_code_versions as the canary — it's the first call and the
    // one that previously panicked on short responses.
    let code_versions = {
        let mut cv = None;
        for attempt in 1..=30 {
            match amt::get_code_versions(heci) {
                Ok(v) => {
                    dprintln!("do_amt_info: GetCodeVersions ok on attempt {}", attempt);
                    cv = Some(v);
                    break;
                }
                Err(e) => {
                    dprintln!("do_amt_info: GetCodeVersions attempt {} failed: {:?}", attempt, e);
                    ui::show_working("AMT not ready, retrying...");
                    // Reset HECI between attempts — driver state may be stuck
                    heci.close();
                    let _ = unsafe { heci.init() };
                    let _ = heci.connect_amthi();
                    sleep_ms(2000);
                }
            }
        }
        cv
    };

    let mode = amt::get_control_mode(heci).ok();
    let prov_state = amt::get_provisioning_state(heci).ok();
    let uuid = amt::get_uuid(heci).ok();
    let dns_suffix = amt::get_dns_suffix(heci).ok();
    let wired_lan = amt::get_lan_interface_settings(heci, false).ok();
    let wireless_lan = amt::get_lan_interface_settings(heci, true).ok();
    let lsa = amt::get_local_system_account(heci).ok();

    ui::clear();
    ui::v_center(30);
    ui::box_top();
    ui::box_center("AMT Device Information");
    ui::box_sep();
    ui::box_blank();

    // Firmware info from CodeVersions
    if let Some(ref cv) = code_versions {
        if let Some(v) = cv.find(b"AMT") {
            let s = core::str::from_utf8(v).unwrap_or("?");
            ui::box_kv("FW Version", s);
        }
        if let Some(v) = cv.find(b"Build Number") {
            let s = core::str::from_utf8(v).unwrap_or("?");
            ui::box_kv("Build Number", s);
        }
        if let Some(v) = cv.find(b"Sku") {
            let s = core::str::from_utf8(v).unwrap_or("?");
            ui::box_kv("SKU", s);
        }
    }

    match mode {
        Some(m) => ui::box_kv("Control Mode", amt::control_mode_str(m)),
        None => ui::box_kv("Control Mode", "(error)"),
    }

    if let Some(state) = prov_state {
        let state_str = match state {
            0 => "Pre-Provisioning",
            1 => "In Provisioning",
            2 => "Post-Provisioning",
            _ => "Unknown",
        };
        ui::box_kv("Prov. State", state_str);
    }

    if let Some(uuid) = uuid {
        let mut buf = [0u8; 36];
        format_uuid(&uuid, &mut buf);
        let uuid_str = core::str::from_utf8(&buf).unwrap_or("?");
        ui::box_kv("UUID", uuid_str);
    }

    if let Some((suffix, len)) = dns_suffix {
        if len > 0 {
            let s = core::str::from_utf8(&suffix[..len]).unwrap_or("?");
            ui::box_kv("DNS Suffix", s);
        }
    }

    if let Some(ref lan) = wired_lan {
        show_lan("--- Wired Adapter ---", lan);
    }
    if let Some(ref lan) = wireless_lan {
        show_lan("--- Wireless Adapter ---", lan);
    }

    // LSA
    if let Some(ref lsa) = lsa {
        ui::box_blank();
        ui::box_line("--- Local System Account ---");
        let username = core::str::from_utf8(
            &lsa.username[..lsa.username.iter().position(|&b| b == 0).unwrap_or(0)]
        ).unwrap_or("?");
        ui::box_kv("Username", username);

        let pwd_len = lsa.password.iter().position(|&b| b == 0).unwrap_or(lsa.password.len());
        let mut pwd_msg = [0u8; 24];
        let n = format_pwd_len(&mut pwd_msg, pwd_len);
        let pwd_str = core::str::from_utf8(&pwd_msg[..n]).unwrap_or("?");
        ui::box_kv("Password", pwd_str);
    }

    ui::box_blank();
    ui::press_any_key();
}

fn format_ipv4(addr: &[u8; 4], buf: &mut [u8; 16]) -> usize {
    let mut p = 0;
    for (i, &octet) in addr.iter().enumerate() {
        if i > 0 { buf[p] = b'.'; p += 1; }
        if octet >= 100 { buf[p] = b'0' + octet / 100; p += 1; }
        if octet >= 10 { buf[p] = b'0' + (octet / 10) % 10; p += 1; }
        buf[p] = b'0' + octet % 10; p += 1;
    }
    p
}

fn format_mac(addr: &[u8; 6], buf: &mut [u8; 18]) {
    let hex = b"0123456789abcdef";
    let mut p = 0;
    for (i, &b) in addr.iter().enumerate() {
        if i > 0 { buf[p] = b':'; p += 1; }
        buf[p] = hex[(b >> 4) as usize]; p += 1;
        buf[p] = hex[(b & 0x0f) as usize]; p += 1;
    }
}

fn show_lan(header: &str, lan: &amt::LanInterfaceSettings) {
    ui::box_blank();
    ui::box_line(header);
    ui::box_kv("Link Status", if lan.link_status == 1 { "up" } else { "down" });
    ui::box_kv("DHCP", if lan.dhcp_enabled { "Enabled" } else { "Disabled" });

    let mut ip_buf = [0u8; 16];
    let ip_len = format_ipv4(&lan.ipv4_addr, &mut ip_buf);
    let ip_str = core::str::from_utf8(&ip_buf[..ip_len]).unwrap_or("?");
    ui::box_kv("IPv4 Address", ip_str);

    let mut mac_buf = [0u8; 18];
    format_mac(&lan.mac_addr, &mut mac_buf);
    let mac_str = core::str::from_utf8(&mac_buf[..17]).unwrap_or("?");
    ui::box_kv("MAC Address", mac_str);
}

fn format_uuid(uuid: &[u8; 16], buf: &mut [u8; 36]) {
    let hex = b"0123456789abcdef";
    let order: [usize; 16] = [3,2,1,0, 5,4, 7,6, 8,9, 10,11,12,13,14,15];
    let mut p = 0;
    for (i, &idx) in order.iter().enumerate() {
        if i == 4 || i == 6 || i == 8 || i == 10 {
            buf[p] = b'-';
            p += 1;
        }
        let b = uuid[idx];
        buf[p] = hex[(b >> 4) as usize];
        buf[p + 1] = hex[(b & 0x0f) as usize];
        p += 2;
    }
}

fn format_pwd_len(buf: &mut [u8; 24], len: usize) -> usize {
    let mut p = 0;
    buf[p] = b'('; p += 1;
    // Write number
    if len == 0 {
        buf[p] = b'0'; p += 1;
    } else {
        let mut digits = [0u8; 10];
        let mut n = len;
        let mut d = 0;
        while n > 0 {
            digits[d] = (n % 10) as u8 + b'0';
            n /= 10;
            d += 1;
        }
        for i in (0..d).rev() {
            buf[p] = digits[i];
            p += 1;
        }
    }
    let suffix = b" chars)";
    buf[p..p + suffix.len()].copy_from_slice(suffix);
    p += suffix.len();
    p
}

fn do_deactivate(heci: &mut HeciContext) {
    ui::clear();
    ui::v_center(10);
    ui::box_top();
    ui::box_center("Deactivate AMT");
    ui::box_sep();
    ui::box_blank();

    let control_mode = match amt::get_control_mode(heci) {
        Ok(m) => m,
        Err(_) => {
            ui::box_line("Error reading control mode.");
            ui::box_blank();
            ui::press_any_key();
            return;
        }
    };

    if control_mode == AMT_CONTROL_MODE_PRE_PROVISIONING {
        ui::box_line("Device is already in Pre-Provisioning state.");
        ui::box_line("Nothing to deactivate.");
        ui::box_blank();
        ui::press_any_key();
        return;
    }

    if control_mode == AMT_CONTROL_MODE_ACM {
        ui::box_line("Device is in Admin Control Mode (ACM).");
        ui::box_line("Cannot deactivate ACM from here.");
        ui::box_blank();
        ui::press_any_key();
        return;
    }

    ui::box_kv("Current Mode", amt::control_mode_str(control_mode));
    ui::box_blank();
    ui::box_line("Are you sure you want to deactivate? (y/n)");
    ui::box_blank();
    ui::box_bottom();

    let ch = ui::wait_key();

    if ch != 'y' && ch != 'Y' {
        return;
    }

    ui::show_working("Deactivating...");

    let result = amt::unprovision(heci);

    // Re-init HECI — unprovision causes ME to reset client connections
    let _ = unsafe { heci.init() };
    let _ = heci.connect_amthi();

    ui::clear();
    ui::v_center(10);
    ui::box_top();
    ui::box_center("Deactivation Result");
    ui::box_sep();
    ui::box_blank();

    match result {
        Ok(_) => {
            ui::box_kv("Status", "SUCCESS");
            if let Ok(mode) = amt::get_control_mode(heci) {
                ui::box_kv("New Mode", amt::control_mode_str(mode));
            }
        }
        Err(e) => {
            let mut msg = [0u8; 40];
            let n = format_error(&mut msg, e);
            let s = core::str::from_utf8(&msg[..n]).unwrap_or("FAILED");
            ui::box_kv("Status", s);
        }
    }

    ui::box_blank();
    ui::press_any_key();
}

fn do_set_dns_suffix(heci: &mut HeciContext) {
    ui::clear();
    ui::v_center(14);
    ui::box_top();
    ui::box_center("Set PKI DNS Suffix");
    ui::box_sep();
    ui::box_blank();

    if let Ok((cur, len)) = amt::get_dns_suffix(heci) {
        if len > 0 {
            let s = core::str::from_utf8(&cur[..len]).unwrap_or("?");
            ui::box_kv("Current", s);
        } else {
            ui::box_kv("Current", "(not set)");
        }
        ui::box_blank();
    }

    ui::box_line("Enter new DNS suffix (ASCII, empty = clear):");
    ui::box_line("Press Enter to confirm, Backspace to edit.");
    ui::box_blank();
    ui::box_bottom();

    let mut buf = [0u8; amt::MAX_DNS_SUFFIX_LEN];
    let len = ui::read_line(&mut buf);

    ui::show_working("Setting DNS suffix...");
    let result = amt::set_dns_suffix(heci, &buf[..len]);

    ui::clear();
    ui::v_center(10);
    ui::box_top();
    ui::box_center("Set DNS Suffix Result");
    ui::box_sep();
    ui::box_blank();

    match result {
        Ok(_) => {
            ui::box_kv("Status", "SUCCESS");
            if let Ok((cur, n)) = amt::get_dns_suffix(heci) {
                if n > 0 {
                    let s = core::str::from_utf8(&cur[..n]).unwrap_or("?");
                    ui::box_kv("New Value", s);
                } else {
                    ui::box_kv("New Value", "(cleared)");
                }
            }
        }
        Err(e) => {
            let mut msg = [0u8; 40];
            let n = format_error(&mut msg, e);
            let s = core::str::from_utf8(&msg[..n]).unwrap_or("FAILED");
            ui::box_kv("Status", s);
        }
    }

    ui::box_blank();
    ui::press_any_key();
}

fn do_activate_ccm(heci: &mut HeciContext) {
    dprintln!("=== CCM ACTIVATE START ===");
    #[cfg(feature = "uefi-target")]
    flush_log();

    ui::clear();
    ui::v_center(10);
    ui::box_top();
    ui::box_center("Activate CCM");
    ui::box_sep();
    ui::box_blank();

    let control_mode = match amt::get_control_mode(heci) {
        Ok(m) => m,
        Err(_) => {
            ui::box_line("Error reading control mode.");
            ui::box_blank();
            ui::press_any_key();
            return;
        }
    };

    if control_mode != AMT_CONTROL_MODE_PRE_PROVISIONING {
        ui::box_line("Device is already activated.");
        ui::box_kv("Current Mode", amt::control_mode_str(control_mode));
        ui::box_line("Deactivate first before re-activating.");
        ui::box_blank();
        ui::press_any_key();
        return;
    }

    ui::box_line("Getting Local System Account...");
    ui::box_bottom();

    let lsa = match amt::get_local_system_account(heci) {
        Ok(l) => l,
        Err(_) => {
            ui::clear();
            ui::v_center(9);
            ui::box_top();
            ui::box_center("Activate CCM");
            ui::box_sep();
            ui::box_blank();
            ui::box_line("Failed to get Local System Account.");
            ui::box_blank();
            ui::press_any_key();
            return;
        }
    };

    dprintln!("=== GOT LSA, STARTING LME ===");
    #[cfg(feature = "uefi-target")]
    flush_log();

    // Show working BEFORE opening LME — screen drawing is slow in UEFI
    // and would cause the APF channel to timeout if done after channel_open.
    ui::show_working("Activating CCM...");

    // Tear down AMTHI and open a fresh HECI context for LME.
    heci.close();
    let lme_heci = match crate::heci::HeciContext::new() {
        Ok(h) => h,
        Err(e) => {
            dprintln!("=== HECI init for LME FAILED: {:?} ===", e);
            #[cfg(feature = "uefi-target")]
            flush_log();

            let _ = unsafe { heci.init() };
            let _ = heci.connect_amthi();

            ui::clear();
            ui::v_center(9);
            ui::box_top();
            ui::box_center("Activate CCM");
            ui::box_sep();
            ui::box_blank();
            ui::box_line("Failed to initialize HECI for LME.");
            ui::box_blank();
            ui::press_any_key();
            return;
        }
    };

    let mut lme_heci = lme_heci;
    if let Err(e) = lme_heci.connect_client(&wsman_apf::message::LME_UUID) {
        dprintln!("=== LME connect FAILED: {:?} ===", e);
        #[cfg(feature = "uefi-target")]
        flush_log();

        lme_heci.close();
        let _ = unsafe { heci.init() };
        let _ = heci.connect_amthi();

        ui::clear();
        ui::v_center(9);
        ui::box_top();
        ui::box_center("Activate CCM");
        ui::box_sep();
        ui::box_blank();
        ui::box_line("Failed to connect LME client.");
        ui::box_blank();
        ui::press_any_key();
        return;
    }

    #[cfg(feature = "uefi-target")]
    let me_addr = lme_heci.me_addr;
    #[cfg(not(feature = "uefi-target"))]
    let me_addr: u8 = 0;
    #[cfg(feature = "uefi-target")]
    let host_addr = lme_heci.host_addr;
    #[cfg(not(feature = "uefi-target"))]
    let host_addr: u8 = 0;
    let transport = AppHeciTransport::new(lme_heci);
    let mut lme = ApfSession::new(transport, AppHeciHooks, me_addr, host_addr);

    dprintln!("=== APF HANDSHAKE ===");
    #[cfg(feature = "uefi-target")]
    flush_log();
    if let Err(e) = lme.handshake() {
        dprintln!("=== APF HANDSHAKE FAILED: {:?} ===", e);
        #[cfg(feature = "uefi-target")]
        flush_log();

        lme.close();
        let _ = unsafe { heci.init() };
        let _ = heci.connect_amthi();

        ui::clear();
        ui::v_center(9);
        ui::box_top();
        ui::box_center("Activate CCM");
        ui::box_sep();
        ui::box_blank();
        ui::box_line("Failed APF handshake.");
        ui::box_blank();
        ui::press_any_key();
        return;
    }
    dprintln!("=== APF HANDSHAKE OK, CHANNEL OPEN ===");
    #[cfg(feature = "uefi-target")]
    flush_log();

    if let Err(e) = lme.channel_open() {
        dprintln!("=== CHANNEL OPEN FAILED: {:?} ===", e);

        lme.close();
        let _ = unsafe { heci.init() };
        let _ = heci.connect_amthi();

        #[cfg(feature = "uefi-target")]
        flush_log();

        ui::clear();
        ui::v_center(9);
        ui::box_top();
        ui::box_center("Activate CCM");
        ui::box_sep();
        ui::box_blank();
        ui::box_line("Failed to open APF channel.");
        ui::box_blank();
        ui::press_any_key();
        return;
    }

    dprintln!("=== CHANNEL OPEN OK, WSMAN ===");

    let result = wsman::activate_ccm(&mut lme, &lsa, DEFAULT_AMT_PASSWORD);

    lme.close();

    // Always re-init HECI after LME session
    let _ = unsafe { heci.init() };
    let _ = heci.connect_amthi();

    ui::clear();
    ui::v_center(11);
    ui::box_top();
    ui::box_center("CCM Activation Result");
    ui::box_sep();
    ui::box_blank();

    match result {
        Ok(ccm_result) => {
            ui::box_kv("Status", "SUCCESS");

            let realm = core::str::from_utf8(
                &ccm_result.digest_realm[..str_util::ascii_len(&ccm_result.digest_realm)]
            ).unwrap_or("?");
            ui::box_kv("DigestRealm", realm);

            if let Ok(mode) = amt::get_control_mode(heci) {
                ui::box_kv("Verified Mode", amt::control_mode_str(mode));
            }
        }
        Err(e) => {
            let mut msg = [0u8; 40];
            let n = format_error(&mut msg, e);
            let s = core::str::from_utf8(&msg[..n]).unwrap_or("FAILED");
            ui::box_kv("Status", s);
        }
    }

    ui::box_blank();
    #[cfg(feature = "uefi-target")]
    flush_log();
    ui::press_any_key();
}

fn format_error(buf: &mut [u8; 40], e: crate::error::Error) -> usize {
    use crate::error::Error;
    let mut len = 0;
    let fixed = match e {
        Error::Timeout => Some("FAILED (Timeout)"),
        Error::NotFound => Some("FAILED (Not Found)"),
        Error::AccessDenied => Some("FAILED (Access Denied)"),
        Error::DeviceError => Some("FAILED (Device Error)"),
        Error::ProtocolError => Some("FAILED (Protocol Error)"),
        Error::BufferTooSmall => Some("FAILED (Buffer Too Small)"),
        Error::Unsupported => Some("FAILED (Unsupported)"),
        Error::Aborted => Some("FAILED (Aborted)"),
        Error::AmtStatus(_) | Error::HttpStatus(_) => None,
    };
    if let Some(msg) = fixed {
        len = msg.len().min(buf.len());
        buf[..len].copy_from_slice(&msg.as_bytes()[..len]);
        return len;
    }

    let (prefix, code) = match e {
        Error::AmtStatus(c) => ("FAILED (AMT ", c),
        Error::HttpStatus(c) => ("FAILED (HTTP ", c),
        _ => unreachable!(),
    };
    for &b in prefix.as_bytes() {
        if len < buf.len() { buf[len] = b; len += 1; }
    }
    let mut tmp = [0u8; 10];
    let mut n = code;
    let mut i = 0;
    if n == 0 { tmp[0] = b'0'; i = 1; }
    while n > 0 { tmp[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
    while i > 0 {
        i -= 1;
        if len < buf.len() { buf[len] = tmp[i]; len += 1; }
    }
    if len < buf.len() { buf[len] = b')'; len += 1; }
    len
}

fn app_main(heci: &mut HeciContext) {
    loop {
        ui::clear();
        ui::v_center(19);

        let mode_str = match amt::get_control_mode(heci) {
            Ok(m) => amt::control_mode_str(m),
            Err(_) => "Unknown",
        };

        ui::box_top();
        ui::box_blank();
        ui::box_center("AMT Configuration Tool");
        ui::box_blank();
        ui::box_sep();
        ui::box_blank();
        ui::box_kv("Current Status", mode_str);
        ui::box_blank();
        ui::box_sep();
        ui::box_blank();
        ui::box_line("  A)  AMT Info");
        ui::box_line("  B)  Deactivate (CCM Unprovision)");
        ui::box_line("  C)  Activate (CCM)");
        ui::box_line("  D)  Set PKI DNS Suffix");
        ui::box_line("  Q)  Quit");
        ui::box_blank();
        ui::box_sep();
        ui::box_line("Select: ");
        ui::box_bottom();

        ui::show_cursor();
        let ch = ui::wait_key();

        match ch {
            'a' | 'A' => do_amt_info(heci),
            'b' | 'B' => do_deactivate(heci),
            'c' | 'C' => do_activate_ccm(heci),
            'd' | 'D' => do_set_dns_suffix(heci),
            'q' | 'Q' => {
                heci.close();
                return;
            }
            _ => {}
        }
    }
}

#[cfg(feature = "uefi-target")]
#[entry]
fn uefi_main() -> Status {
    // Disable the UEFI boot-services watchdog. Default firmware arms a
    // 5-minute watchdog that resets the platform if the app doesn't call
    // ExitBootServices in time. We're an interactive tool — user idle at
    // the menu can easily exceed that.
    let _ = uefi::boot::set_watchdog_timer(0, 0x10000, None);

    ui::init();
    ui::v_center(8);
    ui::box_top();
    ui::box_blank();
    ui::box_center("AMT Configuration Tool");
    ui::box_blank();
    ui::box_sep();
    ui::box_blank();
    ui::box_line("Initializing HECI...");
    ui::box_bottom();

    let mut heci = match HeciContext::new() {
        Ok(h) => h,
        Err(_) => {
            ui::box_line("FAILED: Could not initialize HECI.");
            flush_log();
            ui::press_any_key();
            return Status::DEVICE_ERROR;
        }
    };
    if let Err(_) = heci.connect_amthi() {
        ui::box_line("FAILED: Could not connect to AMTHI.");
        flush_log();
        ui::press_any_key();
        return Status::DEVICE_ERROR;
    }
    app_main(&mut heci);
    flush_log();
    Status::SUCCESS
}

#[cfg(any(feature = "windows-target", feature = "linux-target"))]
fn main() {
    ui::init();
    println!("AMT Configuration Tool");
    println!("Initializing HECI (MEI driver)...");

    let mut heci = match HeciContext::new() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("FAILED: Could not open MEI device ({:?})", e);
            #[cfg(feature = "windows-target")]
            eprintln!("Ensure Intel MEI driver is installed and you are running as Administrator.");
            #[cfg(feature = "linux-target")]
            eprintln!("Ensure the mei/mei_me kernel module is loaded and /dev/mei0 is accessible (try running as root or adding your user to the appropriate group).");
            return;
        }
    };
    if let Err(e) = heci.connect_amthi() {
        eprintln!("FAILED: Could not connect to AMTHI ({:?})", e);
        return;
    }
    println!("Connected to AMTHI.");
    app_main(&mut heci);
}
