use core::arch::asm;

const PCI_CONFIG_ADDR: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// Build a PCI configuration address for a given BDF + register offset.
pub fn pci_addr(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    (1u32 << 31)
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC)
}

/// Read a 32-bit value from PCI configuration space.
///
/// # Safety
/// Caller must ensure `addr` is a valid PCI config address built with `pci_addr`.
pub unsafe fn config_read32(addr: u32) -> u32 {
    let val: u32;
    unsafe {
        asm!(
            "out dx, eax",
            in("eax") addr,
            in("dx") PCI_CONFIG_ADDR,
            options(nomem, nostack, preserves_flags),
        );
        asm!(
            "in eax, dx",
            out("eax") val,
            in("dx") PCI_CONFIG_DATA,
            options(nomem, nostack, preserves_flags),
        );
    }
    val
}

/// Read a 16-bit value from PCI configuration space (low 16 bits of the dword).
///
/// # Safety
/// Caller must ensure `addr` is a valid PCI config address.
pub unsafe fn config_read16(addr: u32) -> u16 {
    unsafe { config_read32(addr) as u16 }
}

/// Write a 16-bit value to PCI configuration space.
///
/// # Safety
/// Caller must ensure `addr` is a valid PCI config address.
pub unsafe fn config_write16(addr: u32, val: u16) {
    unsafe {
        asm!(
            "out dx, eax",
            in("eax") addr,
            in("dx") PCI_CONFIG_ADDR,
            options(nomem, nostack, preserves_flags),
        );
        asm!(
            "out dx, ax",
            in("ax") val,
            in("dx") PCI_CONFIG_DATA,
            options(nomem, nostack, preserves_flags),
        );
    }
}
