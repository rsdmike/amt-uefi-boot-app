// Platform-specific HECI implementations
#[cfg(feature = "windows-target")]
mod windows;
#[cfg(feature = "windows-target")]
pub use self::windows::*;

#[cfg(feature = "uefi-target")]
pub mod regs;
#[cfg(feature = "uefi-target")]
pub mod hbm;

#[cfg(feature = "uefi-target")]
use crate::error::{Error, Result};
#[cfg(feature = "uefi-target")]
use crate::pci;
#[cfg(feature = "uefi-target")]
use regs::*;
#[cfg(feature = "uefi-target")]
use hbm::*;

#[cfg(feature = "uefi-target")]
pub struct HeciContext {
    mmio: *mut u32,
    pub me_addr: u8,
    pub host_addr: u8,
    pub max_msg_len: u32,
    cb_depth: u8,
}

#[cfg(feature = "uefi-target")]
impl HeciContext {
    /// Discover HECI PCI device, map BAR0, and reset the interface.
    ///
    /// # Safety
    /// This performs raw PCI config I/O and MMIO mapping.
    pub fn new() -> Result<Self> {
        let mut ctx = HeciContext {
            mmio: core::ptr::null_mut(),
            me_addr: 0,
            host_addr: 0,
            max_msg_len: 0,
            cb_depth: 0,
        };
        unsafe { ctx.init()? };
        Ok(ctx)
    }

    /// Re-initialize: find PCI device, map BAR0, reset.
    ///
    /// # Safety
    /// Performs raw PCI and MMIO access.
    pub unsafe fn init(&mut self) -> Result<()> {
        self.mmio = core::ptr::null_mut();
        self.me_addr = 0;
        self.host_addr = 0;

        let base_addr = pci::pci_addr(HECI_BUS, HECI_DEVICE, HECI_FUNCTION, 0);

        // Read Vendor ID
        let vendor_id = unsafe { pci::config_read16(base_addr | PCI_VENDOR_ID as u32) };
        if vendor_id != INTEL_VENDOR_ID {
            dprintln!("HECI: Intel device not found at 00:16.0 (vendor=0x{:04x})", vendor_id);
            return Err(Error::NotFound);
        }
        dprintln!("HECI: Vendor ID=0x{:04x} (Intel)", vendor_id);

        // Read BAR0
        let bar0 = unsafe { pci::config_read32(base_addr | PCI_BAR0 as u32) };
        if bar0 == 0 || bar0 == 0xFFFF_FFFF {
            dprintln!("HECI: BAR0 not configured (0x{:08x})", bar0);
            return Err(Error::NotFound);
        }

        // Enable Memory Space access if not set
        let cmd = unsafe { pci::config_read16(base_addr | PCI_COMMAND as u32) };
        dprintln!("HECI: PCI CMD=0x{:04x} (MSE={})", cmd, if cmd & PCI_CMD_MSE != 0 { 1 } else { 0 });

        if cmd & PCI_CMD_MSE == 0 {
            unsafe { pci::config_write16(base_addr | PCI_COMMAND as u32, cmd | PCI_CMD_MSE) };
            dprintln!("HECI: Enabled Memory Space access");
        }

        // Get MMIO base address
        let mut mmio_base: u64 = (bar0 & 0xFFFF_FFF0) as u64;

        // Check if 64-bit BAR
        if (bar0 & 0x06) == 0x04 {
            let bar0_hi = unsafe { pci::config_read32(base_addr | (PCI_BAR0 as u32 + 4)) };
            mmio_base |= (bar0_hi as u64) << 32;
            dprintln!("HECI: 64-bit BAR, BAR0=0x{:08x} BAR1=0x{:08x} -> 0x{:016x}", bar0, bar0_hi, mmio_base);
        } else {
            dprintln!("HECI: 32-bit BAR, base=0x{:016x}", mmio_base);
        }

        self.mmio = mmio_base as usize as *mut u32;

        dprintln!("HECI: Found at PCI 00:16.0, BAR0=0x{:016x}", mmio_base);

        // Reset HECI and wait for ME ready
        self.reset()?;

        dprintln!("HECI: ME ready, CB depth={}", self.cb_depth);

        Ok(())
    }

    // MMIO register access

    fn reg_read(&self, offset: u32) -> u32 {
        unsafe { core::ptr::read_volatile(self.mmio.add((offset / 4) as usize)) }
    }

    fn reg_write(&self, offset: u32, val: u32) {
        unsafe { core::ptr::write_volatile(self.mmio.add((offset / 4) as usize) as *mut u32, val) }
    }

    fn stall(us: u64) {
        uefi::boot::stall(us as usize);
    }

    fn wait_me_ready(&self) -> Result<()> {
        let mut elapsed: u64 = 0;
        while elapsed < HECI_TIMEOUT_US {
            let me_csr = self.reg_read(ME_CSR_HA);
            if me_csr & CSR_RDY != 0 {
                return Ok(());
            }
            Self::stall(HECI_POLL_US);
            elapsed += HECI_POLL_US;
        }
        dprintln!("HECI: Timeout waiting for ME ready");
        Err(Error::Timeout)
    }

    fn reset(&mut self) -> Result<()> {
        dprintln!("HECI: Resetting host interface...");

        // Assert host reset + interrupt generate
        let mut h_csr = self.reg_read(H_CSR);
        h_csr |= CSR_RST | CSR_IG;
        self.reg_write(H_CSR, h_csr);

        Self::stall(10_000); // 10ms for reset

        // Wait for ME ready
        self.wait_me_ready()?;

        // Clear reset, set ready + IG
        h_csr = self.reg_read(H_CSR);
        h_csr &= !CSR_RST;
        h_csr |= CSR_RDY | CSR_IG;
        self.reg_write(H_CSR, h_csr);

        // Read buffer depth
        h_csr = self.reg_read(H_CSR);
        self.cb_depth = csr_get_cbd(h_csr) as u8;

        Ok(())
    }

    fn wait_slots(&self, needed: u32) -> u32 {
        let mut elapsed: u64 = 0;
        while elapsed < HECI_TIMEOUT_US {
            let h_csr = self.reg_read(H_CSR);
            let me_csr = self.reg_read(ME_CSR_HA);
            if me_csr & CSR_RDY == 0 {
                return 0;
            }
            let wp = csr_get_wp(h_csr);
            let rp = csr_get_rp(h_csr);
            let depth = csr_get_cbd(h_csr);
            let filled = filled_slots(wp, rp, depth);
            let empty = depth - filled - 1;
            if empty >= needed {
                return empty;
            }
            Self::stall(HECI_POLL_US);
            elapsed += HECI_POLL_US;
        }
        0
    }

    fn notify_me(&self) {
        let mut h_csr = self.reg_read(H_CSR);
        h_csr |= CSR_IG;
        h_csr &= !CSR_IS;
        self.reg_write(H_CSR, h_csr);
    }

    /// Low-level write with fragmentation support.
    pub fn write_msg(&self, me_addr: u8, host_addr: u8, data: &[u8], complete: bool) -> Result<()> {
        let len = data.len() as u32;
        let mut sent: u32 = 0;

        while sent < len {
            let empty = self.wait_slots(2);
            if empty == 0 {
                return Err(Error::Timeout);
            }

            let max_payload = (empty - 1) * 4;
            let remaining = len - sent;
            let frag_len = remaining.min(max_payload);
            let is_last = sent + frag_len >= len;

            let hdr = HeciMsgHdr::new(me_addr, host_addr, frag_len as u16, is_last && complete);
            self.reg_write(H_CB_WW, hdr.raw());

            // Write payload dwords
            let dwords = (frag_len + 3) / 4;
            for i in 0..dwords {
                let mut word: u32 = 0;
                let base = (sent + i * 4) as usize;
                let left = frag_len - (i * 4);
                let copy = left.min(4) as usize;
                let word_bytes = word.to_ne_bytes();
                let mut buf = word_bytes;
                for j in 0..copy {
                    buf[j] = data[base + j];
                }
                word = u32::from_ne_bytes(buf);
                self.reg_write(H_CB_WW, word);
            }

            self.notify_me();
            sent += frag_len;

            if !is_last {
                Self::stall(1_000); // 1ms between fragments
            }
        }

        Ok(())
    }

    /// Low-level read with fragment reassembly.
    /// Returns (bytes_read, me_addr, host_addr).
    pub fn read_msg(&self, buf: &mut [u8]) -> Result<(u32, u8, u8)> {
        let mut total_read: u32 = 0;
        let mut first_fragment = true;
        let mut out_me: u8 = 0;
        let mut out_host: u8 = 0;
        let mut msg_done = false;

        while !msg_done {
            let mut elapsed: u64 = 0;

            // Wait for data in ME circular buffer
            loop {
                if elapsed >= HECI_TIMEOUT_US {
                    if total_read > 0 {
                        msg_done = true;
                        break;
                    }
                    dprintln!("  [RECV] TIMEOUT after {}s", HECI_TIMEOUT_US / 1_000_000);
                    return Err(Error::Timeout);
                }
                let me_csr = self.reg_read(ME_CSR_HA);
                let wp = csr_get_wp(me_csr);
                let rp = csr_get_rp(me_csr);
                if wp != rp {
                    break;
                }
                Self::stall(HECI_POLL_US);
                elapsed += HECI_POLL_US;
            }

            if msg_done {
                break;
            }

            // Read header
            let hdr_raw = self.reg_read(ME_CB_RW);
            let hdr = HeciMsgHdr(hdr_raw);

            if first_fragment {
                out_me = hdr.me_addr();
                out_host = hdr.host_addr();
                first_fragment = false;
            }

            let frag_len = hdr.length() as u32;

            if total_read + frag_len > buf.len() as u32 {
                dprintln!("  [RECV] Reassembled msg too large: {} + {} > {}", total_read, frag_len, buf.len());
                // Drain remaining dwords
                let drain = (frag_len + 3) / 4;
                for _ in 0..drain {
                    self.reg_read(ME_CB_RW);
                }
                return Err(Error::BufferTooSmall);
            }

            // Read payload dwords
            let dwords = (frag_len + 3) / 4;
            for i in 0..dwords {
                let word = self.reg_read(ME_CB_RW);
                let left = frag_len - (i * 4);
                let copy = left.min(4) as usize;
                let word_bytes = word.to_ne_bytes();
                for j in 0..copy {
                    buf[(total_read + i * 4) as usize + j] = word_bytes[j];
                }
            }

            total_read += frag_len;

            // Acknowledge this fragment
            let mut h_csr = self.reg_read(H_CSR);
            h_csr |= CSR_IG | CSR_IS;
            self.reg_write(H_CSR, h_csr);

            if hdr.msg_complete() {
                msg_done = true;
            }
        }

        Ok((total_read, out_me, out_host))
    }

    // Flow control

    fn send_flow_control(&self) -> Result<()> {
        let fc = HbmFlowControl {
            cmd: HBM_CMD_FLOW_CONTROL,
            me_addr: self.me_addr,
            host_addr: self.host_addr,
            reserved: [0; 5],
        };
        let bytes = unsafe {
            core::slice::from_raw_parts(
                &fc as *const HbmFlowControl as *const u8,
                core::mem::size_of::<HbmFlowControl>(),
            )
        };
        dprintln!("  [FC] Sending flow control credit for ME:{} Host:{}", self.me_addr, self.host_addr);
        self.write_msg(0, 0, bytes, true)
    }

    /// Send a payload to the connected ME client (with flow control).
    pub fn send(&self, data: &[u8]) -> Result<()> {
        dprintln!("  [heci_send] Sending {} bytes to ME:{}", data.len(), self.me_addr);
        self.write_msg(self.me_addr, self.host_addr, data, true)?;
        self.send_flow_control()
    }

    /// Receive a payload from the connected ME client.
    /// Skips HBM messages (flow control, disconnect requests).
    pub fn receive(&self, buf: &mut [u8]) -> Result<u32> {
        let mut tmp_buf = [0u8; 2048];

        dprintln!("  [heci_recv] Waiting for response (buf={})...", buf.len());

        loop {
            let (tmp_len, msg_me, msg_host) = self.read_msg(&mut tmp_buf)?;

            // HBM message (me=0, host=0) — skip
            if msg_me == 0 && msg_host == 0 {
                let hbm_cmd = tmp_buf[0];
                dprintln!("  [heci_recv] HBM message: cmd=0x{:02x} len={}", hbm_cmd, tmp_len);

                if hbm_cmd == HBM_CMD_FLOW_CONTROL {
                    dprintln!("  [heci_recv] Flow control from ME (for ME:{} Host:{})", tmp_buf[1], tmp_buf[2]);
                } else if hbm_cmd == HBM_CMD_CLIENT_DISCONNECT_REQ && tmp_len >= 4 {
                    dprintln!("  [heci_recv] HBM DISCONNECT REQ: me_addr={} host_addr={} status={}",
                             tmp_buf[1], tmp_buf[2], tmp_buf[3]);
                    let disc_resp: [u8; 4] = [HBM_CMD_CLIENT_DISCONNECT_RESP, tmp_buf[1], tmp_buf[2], 0];
                    let _ = self.write_msg(0, 0, &disc_resp, true);
                    dprintln!("  [heci_recv] Sent disconnect response");
                }
                continue;
            }

            // Check if addressed to us
            if msg_me == self.me_addr && msg_host == self.host_addr {
                dprintln!("  [heci_recv] Got client message: {} bytes", tmp_len);
                let len = tmp_len as usize;
                if len > buf.len() {
                    return Err(Error::BufferTooSmall);
                }
                buf[..len].copy_from_slice(&tmp_buf[..len]);
                return Ok(tmp_len);
            }

            dprintln!("  [heci_recv] Unexpected msg from ME:{} Host:{} (len={}), skipping",
                     msg_me, msg_host, tmp_len);
        }
    }

    /// HBM version exchange, enumerate clients, find AMTHI, connect.
    pub fn connect_amthi(&mut self) -> Result<()> {
        self.connect_client_impl(&AMTHI_UUID, 1)
    }

    /// Connect to any ME client by UUID.
    pub fn connect_client(&mut self, target_uuid: &[u8; 16]) -> Result<()> {
        self.connect_client_impl(target_uuid, 2)
    }

    fn connect_client_impl(&mut self, target_uuid: &[u8; 16], host_addr: u8) -> Result<()> {
        let mut buf = [0u8; 256];

        dprintln!("HECI: Connecting to client UUID: {:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                 target_uuid[3], target_uuid[2], target_uuid[1], target_uuid[0],
                 target_uuid[5], target_uuid[4], target_uuid[7], target_uuid[6],
                 target_uuid[8], target_uuid[9],
                 target_uuid[10], target_uuid[11], target_uuid[12], target_uuid[13], target_uuid[14], target_uuid[15]);

        // Step 1: HBM Version Exchange
        dprintln!("HECI: Step 1 - HBM version exchange");
        {
            let req = HbmHostVersionReq {
                cmd: HBM_CMD_HOST_VERSION,
                minor: HBM_MINOR_VERSION,
                major: HBM_MAJOR_VERSION,
                reserved: 0,
            };
            let req_bytes = unsafe {
                core::slice::from_raw_parts(&req as *const _ as *const u8, core::mem::size_of_val(&req))
            };
            self.write_msg(0, 0, req_bytes, true)?;

            let (len, _, _) = self.read_msg(&mut buf)?;
            if len < 4 {
                return Err(Error::ProtocolError);
            }
            let resp = unsafe { &*(buf.as_ptr() as *const HbmHostVersionResp) };
            if resp.cmd != HBM_CMD_HOST_VERSION_RESP || resp.supported == 0 {
                dprintln!("HECI: HBM version not supported (cmd=0x{:02x} supp={})", resp.cmd, resp.supported);
                return Err(Error::Unsupported);
            }
            dprintln!("HECI: HBM version {}.{} OK", resp.major, resp.minor);
        }

        // Step 2: Enumerate ME clients
        dprintln!("HECI: Step 2 - Enumerate clients");
        let mut valid_addresses = [0u8; 32];
        {
            let req = HbmHostEnumReq {
                cmd: HBM_CMD_HOST_ENUM,
                reserved: [0; 3],
            };
            let req_bytes = unsafe {
                core::slice::from_raw_parts(&req as *const _ as *const u8, core::mem::size_of_val(&req))
            };
            self.write_msg(0, 0, req_bytes, true)?;

            let (len, _, _) = self.read_msg(&mut buf)?;
            if len < 36 {
                return Err(Error::ProtocolError);
            }
            let resp = unsafe { &*(buf.as_ptr() as *const HbmHostEnumResp) };
            if resp.cmd != HBM_CMD_HOST_ENUM_RESP {
                dprintln!("HECI: Enum failed (cmd=0x{:02x})", resp.cmd);
                return Err(Error::DeviceError);
            }
            valid_addresses.copy_from_slice(&resp.valid_addresses);

            let mut client_count: u32 = 0;
            for i in 1u32..256 {
                if valid_addresses[(i / 8) as usize] & (1 << (i % 8)) != 0 {
                    client_count += 1;
                }
            }
            dprintln!("HECI: Found {} ME clients", client_count);
        }

        // Step 3: Query each client to find target UUID
        dprintln!("HECI: Step 3 - Find target client");
        let mut found_me_addr: u8 = 0;
        let mut found = false;

        for i in 1u32..256 {
            if found {
                break;
            }
            if valid_addresses[(i / 8) as usize] & (1 << (i % 8)) == 0 {
                continue;
            }

            let req = HbmClientPropReq {
                cmd: HBM_CMD_HOST_CLIENT_PROP,
                me_addr: i as u8,
                reserved: [0; 2],
            };
            let req_bytes = unsafe {
                core::slice::from_raw_parts(&req as *const _ as *const u8, core::mem::size_of_val(&req))
            };

            if self.write_msg(0, 0, req_bytes, true).is_err() {
                continue;
            }

            match self.read_msg(&mut buf) {
                Ok((len, _, _)) => {
                    if len < core::mem::size_of::<HbmClientPropResp>() as u32 {
                        continue;
                    }
                    let resp = unsafe { &*(buf.as_ptr() as *const HbmClientPropResp) };
                    if resp.cmd != HBM_CMD_HOST_CLIENT_PROP_RESP || resp.status != 0 {
                        continue;
                    }

                    let max_msg = { resp.max_msg_length };
                    dprintln!("  ME[{}]: {:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} (max_msg={})",
                             resp.me_addr,
                             resp.uuid[3], resp.uuid[2], resp.uuid[1], resp.uuid[0],
                             resp.uuid[5], resp.uuid[4], resp.uuid[7], resp.uuid[6],
                             resp.uuid[8], resp.uuid[9],
                             resp.uuid[10], resp.uuid[11], resp.uuid[12], resp.uuid[13], resp.uuid[14], resp.uuid[15],
                             max_msg);

                    if resp.uuid == *target_uuid {
                        found_me_addr = resp.me_addr;
                        self.max_msg_len = max_msg;
                        found = true;
                        dprintln!("  ^^ TARGET MATCH!");
                    }
                }
                Err(_) => continue,
            }
        }

        if !found {
            dprintln!("HECI: Target client not found among enumerated clients");
            return Err(Error::NotFound);
        }

        self.me_addr = found_me_addr;

        // Step 4: Connect
        dprintln!("HECI: Step 4 - Connect to client");
        {
            self.host_addr = host_addr;

            let req = HbmConnectReq {
                cmd: HBM_CMD_CONNECT,
                me_addr: self.me_addr,
                host_addr: self.host_addr,
                reserved: 0,
            };
            let req_bytes = unsafe {
                core::slice::from_raw_parts(&req as *const _ as *const u8, core::mem::size_of_val(&req))
            };
            self.write_msg(0, 0, req_bytes, true)?;

            let (len, _, _) = self.read_msg(&mut buf)?;
            if len < 4 {
                return Err(Error::ProtocolError);
            }
            let resp = unsafe { &*(buf.as_ptr() as *const HbmConnectResp) };
            if resp.cmd != HBM_CMD_CONNECT_RESP || resp.status != 0 {
                dprintln!("HECI: Connect failed (cmd=0x{:02x} status={})", resp.cmd, resp.status);
                return Err(Error::DeviceError);
            }

            dprintln!("HECI: Connected (ME:{} <-> Host:{})", self.me_addr, self.host_addr);
        }

        // Step 5: Consume initial flow control from ME
        dprintln!("HECI: Step 5 - Wait for initial flow control");
        {
            let mut fc_buf = [0u8; 64];
            match self.read_msg(&mut fc_buf) {
                Ok((fc_len, fc_me, fc_host)) => {
                    dprintln!("HECI: Initial msg: me={} host={} cmd=0x{:02x} len={}",
                             fc_me, fc_host, fc_buf[0], fc_len);
                }
                Err(_) => {
                    dprintln!("HECI: Warning - no initial flow control");
                }
            }
        }

        Ok(())
    }

    pub fn close(&mut self) {
        dprintln!("HECI: Closing");
        self.mmio = core::ptr::null_mut();
        self.me_addr = 0;
        self.host_addr = 0;
    }
}
