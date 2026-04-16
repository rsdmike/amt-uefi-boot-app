// HECI PCI location
pub const HECI_BUS: u8 = 0;
pub const HECI_DEVICE: u8 = 22; // 0x16
pub const HECI_FUNCTION: u8 = 0;

// PCI config space offsets
pub const PCI_VENDOR_ID: u8 = 0x00;
pub const PCI_COMMAND: u8 = 0x04;
pub const PCI_BAR0: u8 = 0x10;

pub const PCI_CMD_MSE: u16 = 1 << 1; // Memory Space Enable

pub const INTEL_VENDOR_ID: u16 = 0x8086;

// HECI MMIO register offsets from BAR0
pub const H_CB_WW: u32 = 0x00;   // Host Circular Buffer Write Window
pub const H_CSR: u32 = 0x04;     // Host Control/Status Register
pub const ME_CB_RW: u32 = 0x08;  // ME Circular Buffer Read Window
pub const ME_CSR_HA: u32 = 0x0C; // ME Control/Status Register (Host Access)

// CSR bit fields
// pub const CSR_IE: u32 = 1 << 0;  // Interrupt Enable (unused — for interrupt-driven I/O)
pub const CSR_IS: u32 = 1 << 1;  // Interrupt Status (W1C)
pub const CSR_IG: u32 = 1 << 2;  // Interrupt Generate
pub const CSR_RDY: u32 = 1 << 3; // Ready
pub const CSR_RST: u32 = 1 << 4; // Reset

// CSR field extraction
pub const CSR_RP_SHIFT: u32 = 8;
pub const CSR_WP_SHIFT: u32 = 16;
pub const CSR_CBD_SHIFT: u32 = 24;

pub const CSR_RP_MASK: u32 = 0x0000_FF00;
pub const CSR_WP_MASK: u32 = 0x00FF_0000;
pub const CSR_CBD_MASK: u32 = 0xFF00_0000;

#[inline]
pub fn csr_get_rp(csr: u32) -> u32 {
    (csr & CSR_RP_MASK) >> CSR_RP_SHIFT
}

#[inline]
pub fn csr_get_wp(csr: u32) -> u32 {
    (csr & CSR_WP_MASK) >> CSR_WP_SHIFT
}

#[inline]
pub fn csr_get_cbd(csr: u32) -> u32 {
    (csr & CSR_CBD_MASK) >> CSR_CBD_SHIFT
}

#[inline]
pub fn filled_slots(wp: u32, rp: u32, depth: u32) -> u32 {
    (wp.wrapping_sub(rp)) % depth
}

// Timeouts
pub const HECI_TIMEOUT_US: u64 = 5_000_000; // 5 seconds
pub const HECI_POLL_US: u64 = 1_000;        // 1ms

// pub const HECI_MAX_PAYLOAD: usize = 4096;

/// HECI message header - packed as a u32 on the wire.
///
/// bits [7:0]   - ME address
/// bits [15:8]  - Host address
/// bits [24:16] - Length (9 bits)
/// bits [30:25] - Reserved
/// bit  [31]    - Message Complete flag
#[derive(Clone, Copy)]
pub struct HeciMsgHdr(pub u32);

impl HeciMsgHdr {
    pub fn new(me_addr: u8, host_addr: u8, length: u16, msg_complete: bool) -> Self {
        let val = (me_addr as u32)
            | ((host_addr as u32) << 8)
            | (((length & 0x1FF) as u32) << 16)
            | (if msg_complete { 1u32 << 31 } else { 0 });
        Self(val)
    }

    pub fn me_addr(self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    pub fn host_addr(self) -> u8 {
        ((self.0 >> 8) & 0xFF) as u8
    }

    pub fn length(self) -> u16 {
        ((self.0 >> 16) & 0x1FF) as u16
    }

    pub fn msg_complete(self) -> bool {
        (self.0 >> 31) != 0
    }

    pub fn raw(self) -> u32 {
        self.0
    }
}
