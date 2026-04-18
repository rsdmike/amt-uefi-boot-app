//! `wsman_apf::HeciTransport` impl over our `HeciContext`.
//!
//! One type, three cfg-gated method bodies.
//! - UEFI: drives MMIO directly via `HeciContext::write_msg`/`read_msg`,
//!   manages flow-control credits manually, surfaces HBM_DISCONNECT_REQ
//!   as `HeciError::Io` so `ApfSession::reopen_channel` triggers the
//!   `reconnect_heci` hook.
//! - Windows/Linux: delegates to `HeciContext::send`/`receive`, which
//!   already wrap the platform driver's HBM handling.

#[cfg(feature = "uefi-target")]
extern crate alloc;

#[cfg(feature = "uefi-target")]
use alloc::format;
#[cfg(feature = "uefi-target")]
use alloc::string::ToString;

use wsman_apf::error::HeciError;
use wsman_apf::transport::{HeciHooks, HeciTransport};

use crate::heci::HeciContext;

pub struct AppHeciTransport {
    ctx: HeciContext,
}

impl AppHeciTransport {
    pub fn new(ctx: HeciContext) -> Self {
        Self { ctx }
    }

    #[cfg(feature = "uefi-target")]
    pub fn me_addr(&self) -> u8 {
        self.ctx.me_addr
    }

    #[cfg(feature = "uefi-target")]
    pub fn host_addr(&self) -> u8 {
        self.ctx.host_addr
    }

    /// Borrow back the inner context (UEFI re-init needs raw access).
    pub fn ctx_mut(&mut self) -> &mut HeciContext {
        &mut self.ctx
    }
}

impl HeciTransport for AppHeciTransport {
    #[cfg(feature = "uefi-target")]
    fn send(&mut self, me: u8, host: u8, data: &[u8]) -> Result<(), HeciError> {
        self.ctx
            .write_msg(me, host, data, true)
            .map_err(|e| HeciError::Io(format!("{e:?}")))?;
        // Grant ME a flow control credit so it can reply.
        let _ = self.send_flow_control_credit();
        Ok(())
    }

    #[cfg(any(feature = "windows-target", feature = "linux-target"))]
    fn send(&mut self, _me: u8, _host: u8, data: &[u8]) -> Result<(), HeciError> {
        self.ctx
            .send(data)
            .map_err(|e| HeciError::Io(format!("{e:?}")))
    }

    #[cfg(feature = "uefi-target")]
    fn recv(&mut self, buf: &mut [u8]) -> Result<(usize, u8, u8), HeciError> {
        let mut tmp = [0u8; 2048];
        loop {
            let (len, me, host) = self
                .ctx
                .read_msg(&mut tmp)
                .map_err(|e| HeciError::Io(format!("{e:?}")))?;

            if me == 0 && host == 0 {
                let cmd = tmp[0];
                if cmd == 0x07 {
                    // HBM_CLIENT_DISCONNECT_REQ — ack and surface as Aborted-style error.
                    let resp: [u8; 4] = [0x87, tmp[1], tmp[2], 0];
                    let _ = self.ctx.write_msg(0, 0, &resp, true);
                    return Err(HeciError::Io("HBM disconnect".to_string()));
                }
                // 0x08 flow control or other HBM informational — drop and re-read.
                continue;
            }

            let n = len as usize;
            if buf.len() < n {
                return Err(HeciError::BufferTooSmall);
            }
            buf[..n].copy_from_slice(&tmp[..n]);
            return Ok((n, me, host));
        }
    }

    #[cfg(any(feature = "windows-target", feature = "linux-target"))]
    fn recv(&mut self, buf: &mut [u8]) -> Result<(usize, u8, u8), HeciError> {
        let n = self
            .ctx
            .receive(buf)
            .map_err(|e| HeciError::Io(format!("{e:?}")))?;
        // Platform driver routes to the connected client; addresses are implicit.
        Ok((n as usize, 0, 0))
    }

    fn close(&mut self) {
        self.ctx.close();
    }

    #[cfg(feature = "uefi-target")]
    fn reset(&mut self) -> Result<(), HeciError> {
        // Full HECI reconnect: drop the MMIO mapping, re-discover the PCI
        // device, redo HBM enumerate+connect for the LME client.
        self.ctx.close();
        unsafe { self.ctx.init() }.map_err(|e| HeciError::Io(format!("{e:?}")))?;
        self.ctx
            .connect_client(&wsman_apf::message::LME_UUID)
            .map_err(|e| HeciError::Io(format!("{e:?}")))
    }

    #[cfg(any(feature = "windows-target", feature = "linux-target"))]
    fn reset(&mut self) -> Result<(), HeciError> {
        // Driver handles reconnect transparently. Default no-op is fine,
        // but make it explicit so the trait method is covered on all targets.
        Ok(())
    }
}

#[cfg(feature = "uefi-target")]
impl AppHeciTransport {
    fn send_flow_control_credit(&self) -> Result<(), HeciError> {
        let fc = [0x08u8, self.ctx.me_addr, self.ctx.host_addr, 0, 0, 0, 0, 0];
        self.ctx
            .write_msg(0, 0, &fc, true)
            .map_err(|e| HeciError::Io(format!("{e:?}")))
    }
}

// --- Hooks ---

#[cfg(feature = "uefi-target")]
pub struct AppHeciHooks;

#[cfg(feature = "uefi-target")]
impl HeciHooks for AppHeciHooks {
    fn post_channel_open_send(&mut self) {
        // Load-bearing filesystem write. Without it, ME doesn't reply to
        // CHANNEL_OPEN. See the comment in the old src/lme/mod.rs for the
        // full forensic story.
        crate::flush_log();
    }

    fn reconnect_heci(
        &mut self,
        heci: &mut dyn HeciTransport,
    ) -> Result<(), HeciError> {
        heci.reset()
    }
}

#[cfg(any(feature = "windows-target", feature = "linux-target"))]
pub use wsman_apf::transport::NoHooks as AppHeciHooks;
