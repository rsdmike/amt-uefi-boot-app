---
name: UEFI development lessons learned
description: Key pitfalls and solutions encountered building UEFI apps with gnu-efi on WSL/Windows
type: project
---

## Calling convention mismatch (#GP fault)
UEFI uses Microsoft x64 ABI (args in RCX, RDX, R8, R9). GCC on Linux uses System V ABI (args in RDI, RSI, RDX, RCX). Direct calls to UEFI protocol functions (ConOut->OutputString, etc.) crash with #GP. **Fix:** wrap every UEFI protocol call with `uefi_call_wrapper(func, nargs, ...)`. GNU-EFI's own library functions (InitializeLib, Print) handle this internally and are safe to call directly.

## OVMF firmware path on Ubuntu 24.x (WSL)
The `ovmf` apt package does NOT install `OVMF_CODE.fd` — it installs `OVMF_CODE_4M.fd` (4MB variant) and `OVMF.fd` (combined). The 4M variants require `-pflash`, not `-bios`. **Fix:** use `/usr/share/ovmf/OVMF.fd` with `-bios` flag for QEMU.

## Build toolchain
Working setup: WSL (Ubuntu) with `gnu-efi`, `gcc`, `make`, `qemu-system-x86`, `ovmf` packages. Build chain: gcc → ld → objcopy (ELF shared object → PE32+ .efi binary). Critical compiler flags: `-fshort-wchar` (UCS-2), `-mno-red-zone` (UEFI interrupt safety).

## HBM flow control
After each PTHI response, the ME sends a flow control credit (cmd=0x08) on the HBM channel (me=0, host=0). Also sends one after HBM_CONNECT. The host must: (a) skip these when reading PTHI responses, (b) send a flow control credit to ME before expecting a response. Without this, the second PTHI call reads the stale flow control message instead of the actual response.

## H_CSR write-1-to-clear bits
H_IS (bit 1) in H_CSR is write-1-to-clear. When writing H_CSR to set IG, mask out IS to avoid accidentally clearing it. After reading from ME CB, set both IG and IS (coreboot pattern).

## Struct packing for wire protocols
All HBM structs must use `__attribute__((packed))`. HBM client properties response field order must match Linux kernel `mei_client_properties` (uuid, protocol_ver, max_connections, fixed_address, single_recv_buf, max_msg_length). Use raw byte arrays for UUIDs, not structured GUIDs.

## Secure Boot blocks unsigned .efi binaries
Real vPro machines have Secure Boot enabled by default. Unsigned UEFI apps are silently skipped. Must disable Secure Boot in BIOS to test.

**Why:** These are non-obvious gotchas that cost debugging time.
**How to apply:** Reference when building or debugging any UEFI application in this project.
