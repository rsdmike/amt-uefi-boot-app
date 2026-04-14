# Building & Running on Windows

The `main.c` source is the same regardless of build method.
Below are two approaches — pick whichever fits your setup.

---

## Option 1: WSL (Recommended — simplest path)

### One-time setup
```powershell
# In PowerShell (admin) — install WSL + Ubuntu if you haven't already
wsl --install
```

Then inside WSL:
```bash
sudo apt update
sudo apt install gnu-efi gcc make qemu-system-x86 ovmf
```

### Build & run
```bash
# From WSL, navigate to your project
cd /mnt/c/Users/Mike/Development/UEFIapp
make
make run
```

That's it — the Makefile already works as-is under WSL.

---

## Option 2: MSYS2 + MinGW-w64 (native Windows)

### One-time setup

1. Install MSYS2 from https://www.msys2.org
2. Open "MSYS2 UCRT64" terminal and run:
```bash
pacman -S mingw-w64-ucrt-x86_64-gcc make mingw-w64-ucrt-x86_64-gnu-efi
```

### Build
From the MSYS2 UCRT64 terminal:
```bash
cd /c/Users/Mike/Development/UEFIapp

# Compile
gcc -I/ucrt64/include/efi -I/ucrt64/include/efi/x86_64 \
    -ffreestanding -fno-stack-protector -fno-stack-check \
    -fshort-wchar -mno-red-zone -maccumulate-outgoing-args \
    -DEFI_FUNCTION_WRAPPER -Wall -c main.c -o main.o

# Link
ld -nostdlib -znocombreloc \
   -T /ucrt64/lib/elf_x86_64_efi.lds \
   -shared -Bsymbolic \
   -L /ucrt64/lib \
   /ucrt64/lib/crt0-efi-x86_64.o \
   main.o -o main.so -lefi -lgnuefi

# Convert to PE32+ .efi
objcopy -j .text -j .sdata -j .data -j .dynamic \
        -j .dynsym -j .rel -j .rela -j .reloc \
        --target=efi-app-x86_64 main.so hello.efi
```

---

## Running in QEMU on Windows

### Install QEMU
Download from https://www.qemu.org/download/#windows and add to PATH.

### Get OVMF firmware
- Download a prebuilt OVMF from:
  https://retrage.github.io/edk2-nightly/
  (grab `RELEASEX64_OVMF.fd` or the split `OVMF_CODE.fd`)
- Or if using WSL: it's already at `/usr/share/OVMF/OVMF_CODE.fd`

### Create ESP and run (PowerShell)
```powershell
# Create fake EFI System Partition
mkdir -p esp\EFI\BOOT
copy hello.efi esp\EFI\BOOT\BOOTX64.EFI

# Run in QEMU (adjust OVMF path as needed)
qemu-system-x86_64 `
    -bios "C:\path\to\OVMF_CODE.fd" `
    -drive format=raw,file=fat:rw:esp `
    -net none
```

### Run (WSL — easier)
```bash
make run
```
