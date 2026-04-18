//! Caller-side glue between the app and the wsman-rs v2 crates.
//!
//! Public entry points:
//! - `activate_ccm` — GetGeneralSettings then HostBasedSetup. CCM provisioning.
//! - `unprovision_acm` — SetupAndConfigurationService::Unprovision. ACM deactivation.
//!
//! Both share `open_client`, which closes the AMTHI HECI session, opens
//! a fresh one for LME, runs APF handshake + channel_open, and returns a
//! ready-to-use `wsman_core::Client`.

extern crate alloc;

use alloc::string::{String, ToString};

use wsman_amt::general::Settings;
use wsman_amt::hostbasedsetup::{HostBasedSetupService, SetupInput};
use wsman_apf::apf_transport::ApfTransport;
use wsman_apf::session::ApfSession;
use wsman_core::client::{Client, Credentials};

use crate::amt::LsaCredentials;
use crate::error::{Error, Result};
use crate::heci::transport::{AppHeciHooks, AppHeciTransport};
use crate::heci::HeciContext;
use crate::md5;
use crate::str_util::{append_bytes, ascii_append, ascii_len};

pub struct CcmResult {
    pub setup_return: u32,
    pub digest_realm: String,
}

type AppClient = Client<ApfTransport<AppHeciTransport, AppHeciHooks>>;

fn open_client(old_heci: &mut HeciContext, lsa: &LsaCredentials) -> Result<AppClient> {
    // Tear down the AMTHI HECI session and open a fresh one for LME.
    old_heci.close();
    let mut heci = HeciContext::new()?;
    heci.connect_client(&wsman_apf::message::LME_UUID)?;

    #[cfg(feature = "uefi-target")]
    let me_addr = heci.me_addr;
    #[cfg(feature = "uefi-target")]
    let host_addr = heci.host_addr;
    #[cfg(any(feature = "windows-target", feature = "linux-target"))]
    let me_addr = 0u8;
    #[cfg(any(feature = "windows-target", feature = "linux-target"))]
    let host_addr = 0u8;

    let transport = AppHeciTransport::new(heci);
    let mut apf = ApfSession::new(transport, AppHeciHooks, me_addr, host_addr);
    apf.handshake().map_err(|_| Error::DeviceError)?;
    apf.channel_open().map_err(|_| Error::DeviceError)?;

    // Username/password from LSA. Both are NUL-terminated byte arrays.
    let user_len = ascii_len(&lsa.username);
    let pass_len = ascii_len(&lsa.password);
    let user = core::str::from_utf8(&lsa.username[..user_len])
        .map_err(|_| Error::ProtocolError)?;
    let pass = core::str::from_utf8(&lsa.password[..pass_len])
        .map_err(|_| Error::ProtocolError)?;

    Ok(Client::new(
        ApfTransport::new(apf),
        Credentials::digest(user, pass),
    ))
}

/// CCM provisioning over WSMAN: GetGeneralSettings -> HostBasedSetup.
pub fn activate_ccm(
    old_heci: &mut HeciContext,
    lsa: &LsaCredentials,
    admin_password: &[u8],
) -> Result<CcmResult> {
    let mut client = open_client(old_heci, lsa)?;

    let gs = Settings::new(&mut client)
        .get()
        .map_err(map_wsman_err)?;

    // MD5 hash of "admin:<digest_realm>:<admin_password>".
    // Same algorithm as the old src/wsman.rs::hash_admin_password.
    let mut hex = [0u8; 33];
    hash_admin_password(gs.digest_realm.as_bytes(), admin_password, &mut hex);
    let hash = core::str::from_utf8(&hex[..32])
        .map_err(|_| Error::ProtocolError)?
        .to_string();

    let out = HostBasedSetupService::new(&mut client)
        .setup(SetupInput {
            admin_password_hash: hash,
            encryption_type: 2,
        })
        .map_err(map_wsman_err)?;

    if out.return_value != 0 {
        return Err(Error::DeviceError);
    }
    Ok(CcmResult {
        setup_return: out.return_value,
        digest_realm: gs.digest_realm,
    })
}

/// ACM deactivation over WSMAN: SetupAndConfigurationService::Unprovision.
/// PTHI unprovision (in src/amt.rs) only works for CCM; this is the ACM path.
pub fn unprovision_acm(
    old_heci: &mut HeciContext,
    lsa: &LsaCredentials,
) -> Result<u32> {
    use wsman_amt::setupandconfiguration::{
        ProvisioningMode, SetupAndConfigurationService, UnprovisionInput,
    };

    let mut client = open_client(old_heci, lsa)?;
    let out = SetupAndConfigurationService::new(&mut client)
        .unprovision(UnprovisionInput {
            mode: ProvisioningMode::None,
        })
        .map_err(map_wsman_err)?;

    if out.return_value != 0 {
        return Err(Error::DeviceError);
    }
    Ok(out.return_value)
}

fn hash_admin_password(realm: &[u8], password: &[u8], hex_out: &mut [u8; 33]) {
    let mut input = [0u8; 256];
    let mut p = 0;
    p = append_bytes(&mut input, p, b"admin:");
    p = ascii_append(&mut input, p, realm);
    input[p] = b':';
    p += 1;
    p = append_bytes(&mut input, p, password);

    let mut digest = [0u8; 16];
    md5::md5_hash(&input[..p], &mut digest);
    md5::md5_hex(&digest, hex_out);
}

fn map_wsman_err(_e: wsman_core::WsmanError) -> Error {
    // Coarse mapping for now. The existing format_error() helper in main.rs
    // doesn't distinguish wsman fault types from generic device errors.
    Error::DeviceError
}
