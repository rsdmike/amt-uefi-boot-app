pub mod apf;

use crate::error::{Error, Result};
use crate::heci::HeciContext;
use apf::*;

const LME_RX_BUF_SIZE: usize = 4096;

pub struct LmeSession {
    pub heci: HeciContext,
    pub our_channel: u32,
    pub amt_channel: u32,
    pub channel_active: bool, // amt_channel == 0 is a valid channel ID, so track state separately
    pub tx_window: u32,
    pub rx_buf: [u8; LME_RX_BUF_SIZE],
    pub rx_len: u32,
    port_forward_ok: bool, // ME has sent tcpip-forward for port 16992
    #[cfg(feature = "uefi-target")]
    hbm_send_credits: u32, // HBM flow control credits from ME (how many msgs we can send)
}

impl LmeSession {
    /// APF handshake (shared by both platforms).
    ///
    /// Per spec, the sequence after connecting to the LME MEI client is:
    /// 1. Host sends APF_PROTOCOLVERSION (once only — do NOT echo)
    /// 2. ME sends APF_PROTOCOLVERSION back
    /// 3. ME sends APF_SERVICE_REQUEST for "pfwd@amt.intel.com" → we reply SERVICE_ACCEPT
    /// 4. ME sends APF_GLOBAL_REQUEST "tcpip-forward" for port 16992 → we reply REQUEST_SUCCESS
    /// 5. Only AFTER step 4 is the session ready for CHANNEL_OPEN
    fn apf_handshake(&mut self) -> Result<()> {
        // Step 1: Send our protocol version (once — spec says do NOT echo)
        dprintln!("LME: Sending APF protocol version 1.0...");
        let mut pv = [0u8; 93];
        pv[0] = APF_PROTOCOLVERSION;
        write_be32(&mut pv[1..5], 1); // major
        write_be32(&mut pv[5..9], 0); // minor
        write_be32(&mut pv[9..13], 0); // reserved
        self.raw_send(&pv)?;

        // Steps 2-4: Process ME's init messages until we have port forwarding established.
        // Do NOT flush_log here on UEFI — the filesystem I/O introduces enough latency
        // that ME's LME state machine times out the init-to-channel-open sequence.
        dprintln!("LME: Waiting for APF init sequence from ME...");
        let mut resp_buf = [0u8; 512];
        let mut protocol_version_ok = false;
        let mut timeout_count = 0;

        for _attempt in 0..40 {
            match self.raw_recv(&mut resp_buf) {
                Ok(resp_len) => {
                    timeout_count = 0;
                    let msg_type = self.process_apf(&resp_buf[..resp_len as usize]);

                    if msg_type == APF_PROTOCOLVERSION {
                        protocol_version_ok = true;
                        dprintln!("LME: Got ME protocol version");
                    }

                    if self.port_forward_ok {
                        dprintln!("LME: Port forwarding established, init complete");
                        break;
                    }
                }
                Err(_) => {
                    timeout_count += 1;
                    if self.port_forward_ok {
                        break;
                    }
                    if protocol_version_ok && timeout_count >= 2 {
                        dprintln!("LME: Warning: no tcpip-forward received, proceeding anyway");
                        break;
                    }
                    dprintln!("LME: Init recv timeout (count={})", timeout_count);
                }
            }
        }

        dprintln!("LME: APF init result: version={} port_fwd={}",
                 protocol_version_ok as u8, self.port_forward_ok as u8);

        if !protocol_version_ok {
            return Err(Error::Unsupported);
        }

        if !self.port_forward_ok {
            // Protocol handshake completed but ME never sent tcpip-forward.
            // channel_open will still attempt, but the caller should know we're
            // on a degraded path so diagnostic output can distinguish this from
            // a full init failure.
            dprintln!("LME: init degraded — no tcpip-forward from ME");
        }

        Ok(())
    }

    /// True once ME sent tcpip-forward for port 16992 during APF init.
    pub fn port_forwarding_established(&self) -> bool {
        self.port_forward_ok
    }

    /// Initialize LME: close old AMTHI, reset HECI, connect to LME client, APF handshake.
    #[cfg(feature = "uefi-target")]
    pub fn init(old_heci: &mut HeciContext) -> Result<Self> {
        dprintln!("LME: Closing AMTHI and resetting HECI...");
        old_heci.close();

        let mut heci = HeciContext::new()?;
        dprintln!("LME: HeciContext::new() OK");

        dprintln!("LME: Connecting to LME client via HBM...");
        heci.connect_client(&LME_UUID)?;
        dprintln!("LME: connect_client(LME) OK");

        let mut lme = LmeSession {
            heci,
            our_channel: 0,
            amt_channel: 0,
            channel_active: false,
            tx_window: 0,
            rx_buf: [0; LME_RX_BUF_SIZE],
            rx_len: 0,
            port_forward_ok: false,
            hbm_send_credits: 1, // initial FC credit received during HECI connect step 5
        };

        // Grant ME a flow control credit (UEFI only — driver handles this on Windows)
        dprintln!("LME: Sending initial flow control credit...");
        lme.send_flow_control()?;

        lme.apf_handshake()?;
        Ok(lme)
    }

    /// Initialize LME on Windows: open a new MEI device handle for LME client.
    #[cfg(feature = "windows-target")]
    pub fn init(old_heci: &mut HeciContext) -> Result<Self> {
        dprintln!("LME: Opening MEI device for LME client...");
        old_heci.close();

        let mut heci = HeciContext::new()?;
        heci.connect_client(&LME_UUID)?;

        let mut lme = LmeSession {
            heci,
            our_channel: 0,
            amt_channel: 0,
            channel_active: false,
            tx_window: 0,
            rx_buf: [0; LME_RX_BUF_SIZE],
            rx_len: 0,
            port_forward_ok: false,
        };

        lme.apf_handshake()?;
        Ok(lme)
    }

    // --- Platform-specific transport ---

    #[cfg(feature = "uefi-target")]
    fn send_flow_control(&self) -> Result<()> {
        let mut fc = [0u8; 8];
        fc[0] = 0x08;
        fc[1] = self.heci.me_addr;
        fc[2] = self.heci.host_addr;
        self.heci.write_msg(0, 0, &fc, true)
    }

    #[cfg(feature = "uefi-target")]
    fn raw_send(&mut self, data: &[u8]) -> Result<()> {
        // Match the Windows MEI driver behavior: send unconditionally.
        // Previously we blocked here waiting for an HBM flow-control credit
        // from ME — but that loop would silently drop any queued non-HBM
        // APF messages (e.g., the follow-up tcpip-forward GLOBAL_REQUESTs
        // ME sends for ports 16993/623/664), and ME would then reset the
        // platform over the missing REQUEST_SUCCESS replies.
        //
        // The counter is still tracked for diagnostics, but saturates at 0
        // rather than blocking.
        self.hbm_send_credits = self.hbm_send_credits.saturating_sub(1);
        dprintln!("  [lme_send] {} bytes (type=0x{:02x})", data.len(), data[0]);
        self.heci.write_msg(self.heci.me_addr, self.heci.host_addr, data, true)
    }

    #[cfg(feature = "uefi-target")]
    fn raw_recv(&mut self, buf: &mut [u8]) -> Result<u32> {
        let mut tmp_buf = [0u8; 2048];

        loop {
            let (tmp_len, msg_me, msg_host) = self.heci.read_msg(&mut tmp_buf)?;

            if msg_me == 0 && msg_host == 0 {
                let hbm_cmd = tmp_buf[0];
                if hbm_cmd == 0x08 {
                    // HBM flow control credit from ME — track it
                    self.hbm_send_credits += 1;
                    dprintln!("  [lme_recv] HBM flow control (credits={})", self.hbm_send_credits);
                } else if hbm_cmd == 0x07 && tmp_len >= 4 {
                    dprintln!("  [lme_recv] HBM DISCONNECT REQ: me={} host={} status={}",
                             tmp_buf[1], tmp_buf[2], tmp_buf[3]);
                    let resp: [u8; 4] = [0x87, tmp_buf[1], tmp_buf[2], 0];
                    let _ = self.heci.write_msg(0, 0, &resp, true);
                    return Err(Error::Aborted);
                } else {
                    dprintln!("  [lme_recv] HBM cmd=0x{:02x} len={}", hbm_cmd, tmp_len);
                }
                continue;
            }

            if msg_me == self.heci.me_addr && msg_host == self.heci.host_addr {
                let len = tmp_len as usize;
                if len > buf.len() {
                    return Err(Error::BufferTooSmall);
                }
                buf[..len].copy_from_slice(&tmp_buf[..len]);
                let _ = self.send_flow_control();
                return Ok(tmp_len);
            }

            dprintln!("  [lme_recv] Unexpected: me={} host={} len={}", msg_me, msg_host, tmp_len);
        }
    }

    // On Windows, MEI driver handles HBM/flow control — just use send/receive directly
    #[cfg(feature = "windows-target")]
    fn raw_send(&self, data: &[u8]) -> Result<()> {
        self.heci.send(data)
    }

    #[cfg(feature = "windows-target")]
    fn raw_recv(&self, buf: &mut [u8]) -> Result<u32> {
        self.heci.receive(buf)
    }

    /// Process one APF message, update session state. Returns message type.
    fn process_apf(&mut self, data: &[u8]) -> u8 {
        if data.is_empty() {
            return 0;
        }

        let msg_type = data[0];
        let len = data.len();

        match msg_type {
            APF_PROTOCOLVERSION => {
                if len >= 93 {
                    let major = read_be32(&data[1..5]);
                    let minor = read_be32(&data[5..9]);
                    dprintln!("  [APF] Protocol version {}.{}", major, minor);
                }
            }

            APF_CHANNEL_OPEN_CONFIRMATION => {
                if len >= 17 {
                    self.our_channel = read_be32(&data[1..5]);
                    self.amt_channel = read_be32(&data[5..9]);
                    self.tx_window = read_be32(&data[9..13]);
                    self.channel_active = true;
                    dprintln!("  [APF] Channel CONFIRMED: ours={} amt={} window={}",
                             self.our_channel, self.amt_channel, self.tx_window);
                }
            }

            APF_CHANNEL_OPEN_FAILURE => {
                if len >= 9 {
                    let reason = read_be32(&data[5..9]);
                    dprintln!("  [APF] Channel FAILED: reason={}", reason);
                }
            }

            APF_CHANNEL_WINDOW_ADJUST => {
                if len >= 9 {
                    let bytes_to_add = read_be32(&data[5..9]);
                    self.tx_window += bytes_to_add;
                    dprintln!("  [APF] Window adjust +{} (now {})", bytes_to_add, self.tx_window);
                }
            }

            APF_CHANNEL_DATA => {
                if len >= 9 {
                    let data_len = read_be32(&data[5..9]) as usize;
                    dprintln!("  [APF] Channel data: {} bytes", data_len);
                    if data_len > 0 && (self.rx_len as usize + data_len) <= self.rx_buf.len() {
                        let start = self.rx_len as usize;
                        self.rx_buf[start..start + data_len].copy_from_slice(&data[9..9 + data_len]);
                        self.rx_len += data_len as u32;

                        // Send WINDOW_ADJUST to replenish ME's view of our receive window
                        let mut wa = [0u8; 9];
                        wa[0] = APF_CHANNEL_WINDOW_ADJUST;
                        write_be32(&mut wa[1..5], self.amt_channel);
                        write_be32(&mut wa[5..9], data_len as u32);
                        let _ = self.raw_send(&wa);
                    }
                }
            }

            APF_CHANNEL_CLOSE => {
                dprintln!("  [APF] Channel close (was amt_ch={})", self.amt_channel);
                // Respond with channel close
                if self.channel_active {
                    let mut close_resp = [0u8; 5];
                    close_resp[0] = APF_CHANNEL_CLOSE;
                    write_be32(&mut close_resp[1..5], self.amt_channel);
                    let _ = self.raw_send(&close_resp);
                }
                self.channel_active = false;
                self.amt_channel = 0;
                self.tx_window = 0;
            }

            APF_SERVICE_REQUEST => {
                if len >= 5 {
                    let name_len = read_be32(&data[1..5]) as usize;
                    if len >= 5 + name_len {
                        let name = &data[5..5 + name_len];
                        let name_str = core::str::from_utf8(name).unwrap_or("?");
                        dprintln!("  [APF] Service request: '{}'", name_str);

                        // Respond with SERVICE_ACCEPT
                        let mut accept = [0u8; 64];
                        let alen = 5 + name_len;
                        accept[0] = APF_SERVICE_ACCEPT;
                        write_be32(&mut accept[1..5], name_len as u32);
                        let copy = name_len.min(32);
                        accept[5..5 + copy].copy_from_slice(&data[5..5 + copy]);
                        let _ = self.raw_send(&accept[..alen]);
                        dprintln!("  [APF] Sent service accept");
                    }
                }
            }

            APF_GLOBAL_REQUEST => {
                if len >= 5 {
                    let name_len = read_be32(&data[1..5]) as usize;
                    if len >= 6 + name_len {
                        let want_reply = data[5 + name_len];
                        let mut offset = 6 + name_len;

                        // Extract port from tcpip-forward request
                        let mut fwd_port: u32 = 0;
                        if offset + 4 <= len {
                            let addr_len = read_be32(&data[offset..offset + 4]) as usize;
                            offset += 4 + addr_len;
                            if offset + 4 <= len {
                                fwd_port = read_be32(&data[offset..offset + 4]);
                            }
                        }

                        dprintln!("  [APF] Global request: port={} want_reply={}", fwd_port, want_reply);

                        if want_reply != 0 {
                            let mut success = [0u8; 5];
                            success[0] = APF_REQUEST_SUCCESS;
                            write_be32(&mut success[1..5], fwd_port);
                            let _ = self.raw_send(&success);
                            dprintln!("  [APF] Sent request success (port={})", fwd_port);
                        }

                        // Track that ME established port forwarding for HTTP
                        if fwd_port == APF_AMT_HTTP_PORT {
                            self.port_forward_ok = true;
                        }
                    }
                }
            }

            APF_KEEPALIVE_REQUEST => {
                if len >= 5 {
                    let mut reply = [0u8; 5];
                    reply[0] = APF_KEEPALIVE_REPLY;
                    reply[1..5].copy_from_slice(&data[1..5]);
                    let _ = self.raw_send(&reply);
                    dprintln!("  [APF] Keepalive reply");
                }
            }

            _ => {
                dprintln!("  [APF] Unknown type={} len={}", msg_type, len);
            }
        }

        msg_type
    }

    /// Open an APF channel to AMT HTTP port (16992).
    /// Per spec: "forwarded-tcpip" with connected address "127.0.0.1".
    ///
    /// On UEFI, some ME firmware sends HBM_DISCONNECT_REQ instead of
    /// CHANNEL_OPEN_CONFIRMATION when we try to reopen after the previous
    /// channel was closed (typically triggered by HTTP's Connection: close
    /// behavior on the 401 retry path). In that case `try_channel_open`
    /// returns `Err(Aborted)`; we detect it, fully reconnect the LME session,
    /// and retry once.
    pub fn channel_open(&mut self) -> Result<()> {
        let result = self.try_channel_open();

        #[cfg(feature = "uefi-target")]
        if let Err(Error::Aborted) = result {
            dprintln!("LME: channel_open aborted by ME — reconnecting LME session");
            self.reconnect_lme()?;
            return self.try_channel_open();
        }

        result
    }

    /// Single channel_open attempt without retry logic.
    fn try_channel_open(&mut self) -> Result<()> {
        self.our_channel = (self.our_channel + 1) % 32;
        if self.our_channel == 0 {
            self.our_channel = 1;
        }
        self.amt_channel = 0;
        self.tx_window = 0;

        // Build APF_CHANNEL_OPEN message
        // Per spec: channel_type="forwarded-tcpip", connected_addr="127.0.0.1"
        let mut msg = [0u8; 72];
        let mut p = 0;
        msg[p] = APF_CHANNEL_OPEN; p += 1;
        write_be32(&mut msg[p..p+4], 15); p += 4; // "forwarded-tcpip" length
        msg[p..p+15].copy_from_slice(b"forwarded-tcpip"); p += 15;
        write_be32(&mut msg[p..p+4], self.our_channel); p += 4;
        write_be32(&mut msg[p..p+4], LME_RX_WINDOW_SIZE); p += 4;
        write_be32(&mut msg[p..p+4], 0xFFFF_FFFF); p += 4; // reserved
        write_be32(&mut msg[p..p+4], 9); p += 4; // connected addr len = "127.0.0.1"
        msg[p..p+9].copy_from_slice(b"127.0.0.1"); p += 9;
        write_be32(&mut msg[p..p+4], APF_AMT_HTTP_PORT); p += 4;
        write_be32(&mut msg[p..p+4], 9); p += 4; // originator addr len
        msg[p..p+9].copy_from_slice(b"127.0.0.1"); p += 9;
        write_be32(&mut msg[p..p+4], 16992); p += 4; // originator port
        let msg_len = p;

        dprintln!("LME: Sending CHANNEL_OPEN to port {}...", APF_AMT_HTTP_PORT);
        self.raw_send(&msg[..msg_len])?;
        dprintln!("LME: CHANNEL_OPEN raw_send returned, waiting for confirm...");
        // LOAD-BEARING MAGIC: actual filesystem write is required here.
        // Tested negative:
        //   - Pure CPU stall (uefi::boot::stall) — crashes
        //   - TPL raise+drop to drain event notifications — crashes
        //   - Memory fence + small stall — crashes
        // Only an actual filesystem write survives. Hypothesis: the PCIe
        // storage-device traffic from the write flushes something at the
        // root-complex / IOMMU level that ME needs drained before it can
        // respond to our CHANNEL_OPEN over HECI MMIO. We couldn't reproduce
        // the effect with cheaper alternatives, so we keep the flush.
        #[cfg(feature = "uefi-target")]
        crate::flush_log();

        // Wait for confirmation
        let mut resp_buf = [0u8; 512];

        for _attempt in 0..30 {
            match self.raw_recv(&mut resp_buf) {
                Ok(resp_len) => {
                    let msg_type = self.process_apf(&resp_buf[..resp_len as usize]);

                    if msg_type == APF_CHANNEL_OPEN_CONFIRMATION {
                        dprintln!("LME: Channel open! AMT channel={} TX window={}", self.amt_channel, self.tx_window);
                        return Ok(());
                    }

                    if msg_type == APF_CHANNEL_OPEN_FAILURE {
                        dprintln!("LME: Channel open REJECTED by ME");
                        return Err(Error::DeviceError);
                    }

                    // Other messages (window adjust, keepalive, etc.) — keep waiting
                }
                Err(e) => {
                    dprintln!("LME: Channel open recv error {:?}", e);
                    return Err(e);
                }
            }
        }

        dprintln!("LME: Too many non-channel messages during open");
        Err(Error::Timeout)
    }

    /// Reopen a channel after a previous channel was closed.
    ///
    /// On UEFI, some ME firmware sends `HBM_CLIENT_DISCONNECT_REQ` on a plain
    /// channel reopen (and our handling of that disconnect can hard-reset the
    /// platform). The robust pattern is: every time the WSMAN layer needs a
    /// new channel after the previous one was closed, fully reconnect the
    /// LME session first, then open.
    #[cfg(feature = "uefi-target")]
    pub fn reopen_channel(&mut self) -> Result<()> {
        dprintln!("LME: Full reconnect before channel reopen...");
        self.reconnect_lme()?;
        self.try_channel_open()
    }

    #[cfg(feature = "windows-target")]
    pub fn reopen_channel(&mut self) -> Result<()> {
        self.try_channel_open()
    }

    /// Hard reset the LME session: drop HECI, re-init hardware, reconnect to LME
    /// client, redo initial flow-control and APF handshake.
    ///
    /// Used as a recovery path when ME sends HBM_DISCONNECT_REQ instead of
    /// CHANNEL_OPEN_CONFIRMATION on a channel reopen attempt.
    #[cfg(feature = "uefi-target")]
    fn reconnect_lme(&mut self) -> Result<()> {
        self.channel_active = false;
        self.amt_channel = 0;
        self.tx_window = 0;
        self.our_channel = 0;
        self.port_forward_ok = false;
        self.hbm_send_credits = 1;

        self.heci.close();
        // SAFETY: we own the HeciContext and no other access happens here.
        unsafe { self.heci.init() }?;
        self.heci.connect_client(&LME_UUID)?;

        self.send_flow_control()?;
        self.apf_handshake()?;
        Ok(())
    }

    /// Send data through the APF channel.
    pub fn send(&mut self, data: &[u8]) -> Result<()> {
        use core::sync::atomic::{AtomicBool, Ordering};
        static IN_USE: AtomicBool = AtomicBool::new(false);
        if IN_USE.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            dprintln!("LME: send reentered — aborting");
            return Err(Error::Aborted);
        }
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) { IN_USE.store(false, Ordering::Release); }
        }
        let _guard = Guard;

        let total = 9 + data.len();
        static mut SEND_BUF: [u8; 2048] = [0; 2048];
        // SAFETY: IN_USE guard above guarantees no reentrant access
        let send_buf = unsafe { &mut *(&raw mut SEND_BUF) };
        if total > send_buf.len() {
            dprintln!("LME: Send too large ({})", total);
            return Err(Error::BufferTooSmall);
        }

        // APF_CHANNEL_DATA: [94][recipient(4 BE)][length(4 BE)][data...]
        send_buf[0] = APF_CHANNEL_DATA;
        write_be32(&mut send_buf[1..5], self.amt_channel);
        write_be32(&mut send_buf[5..9], data.len() as u32);
        send_buf[9..9 + data.len()].copy_from_slice(data);

        dprintln!("  [LME_SEND] {} bytes payload (APF total={})", data.len(), total);

        self.raw_send(&send_buf[..total])
    }

    /// Receive data from APF channel into rx_buf/rx_len.
    pub fn receive(&mut self, _timeout_ms: u32) -> Result<()> {
        let mut resp_buf = [0u8; 2048];
        self.rx_len = 0;

        for _attempt in 0..100 {
            match self.raw_recv(&mut resp_buf) {
                Ok(resp_len) => {
                    let msg_type = self.process_apf(&resp_buf[..resp_len as usize]);

                    if msg_type == APF_CHANNEL_DATA || msg_type == APF_CHANNEL_WINDOW_ADJUST {
                        continue;
                    }

                    if msg_type == APF_CHANNEL_CLOSE {
                        if self.rx_len > 0 {
                            return Ok(());
                        }
                        return Err(Error::Aborted);
                    }
                }
                Err(_) => {
                    if self.rx_len > 0 {
                        return Ok(());
                    }
                    return Err(Error::Timeout);
                }
            }
        }

        if self.rx_len > 0 {
            Ok(())
        } else {
            Err(Error::Timeout)
        }
    }

    /// Close the APF channel (not HECI). Safe to call multiple times.
    pub fn close_channel(&mut self) {
        if self.channel_active {
            let mut close_msg = [0u8; 5];
            close_msg[0] = APF_CHANNEL_CLOSE;
            write_be32(&mut close_msg[1..5], self.amt_channel);
            let _ = self.raw_send(&close_msg);
            self.channel_active = false;
            self.amt_channel = 0;
            self.tx_window = 0;
        }
    }

    /// Close the APF channel and HECI.
    pub fn close(&mut self) {
        dprintln!("LME: Closing channel");
        self.close_channel();
        self.heci.close();
    }
}
