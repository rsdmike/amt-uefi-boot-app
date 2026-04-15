# UEFI AMT Configuration Tool (Rust)

A bare-metal UEFI application and Windows CLI tool for Intel AMT provisioning via HECI/MEI.

## Features

- AMT Info (control mode, provisioning state, UUID, LSA credentials)
- CCM Activation (Client Control Mode via WSMAN/HostBasedSetup)
- CCM Deactivation (Unprovision)

## Build Targets

### UEFI (boots from USB, no OS required)

```bash
cargo build --target x86_64-unknown-uefi
```

Output: `target/x86_64-unknown-uefi/debug/uefi-amt-provision.efi`

Release build:
```bash
cargo build --release --target x86_64-unknown-uefi
```

Copy the `.efi` file to a FAT32 USB drive at `EFI/BOOT/BOOTX64.EFI` and boot from it.

### Windows (runs in OS, requires Intel MEI driver)

```bash
cargo +stable build --no-default-features --features windows-target --target x86_64-pc-windows-msvc
```

Output: `target/x86_64-pc-windows-msvc/debug/uefi-amt-provision.exe`

Release build:
```bash
cargo +stable build --release --no-default-features --features windows-target --target x86_64-pc-windows-msvc
```

**Requirements:**
- Run as **Administrator** (MEI driver access requires elevation)
- Intel MEI driver must be installed (Intel Management Engine Interface)

## Debug Logging

Set `DEBUG_LOG = true` in `src/main.rs` to enable verbose protocol-level logging from all modules (HECI, LME, APF, HTTP, WSMAN).

## Prerequisites

- Rust nightly toolchain (for UEFI target, configured via `rust-toolchain.toml`)
- Rust stable toolchain (for Windows target): `rustup install stable`
