# UEFI AMT Provisioning App Makefile (GNU-EFI, x86_64)
#
# Prerequisites (Debian/Ubuntu):
#   sudo apt install gnu-efi gcc make
#
# Usage:
#   make          - build BOOTX64.EFI
#   make run      - build and launch in QEMU with OVMF
#   make clean    - remove build artifacts

ARCH         = x86_64
TARGET       = BOOTX64.EFI

# Source files
SRCS         = main.c heci.c amt.c lme.c
OBJS         = $(SRCS:.c=.o)

# GNU-EFI paths (adjust if installed elsewhere)
GNUEFI_INC   = /usr/include/efi
GNUEFI_LIB   = /usr/lib
GNUEFI_CRT   = $(GNUEFI_LIB)/crt0-efi-$(ARCH).o
GNUEFI_LDS   = $(GNUEFI_LIB)/elf_$(ARCH)_efi.lds

# OVMF firmware for QEMU
OVMF         = /usr/share/ovmf/OVMF.fd

# Toolchain
CC           = gcc
LD           = ld
OBJCOPY      = objcopy

# Compiler flags
CFLAGS       = -I$(GNUEFI_INC) \
               -I$(GNUEFI_INC)/$(ARCH) \
               -I. \
               -ffreestanding \
               -fno-stack-protector \
               -fno-stack-check \
               -fshort-wchar \
               -mno-red-zone \
               -maccumulate-outgoing-args \
               -Wall \
               -DEFI_FUNCTION_WRAPPER \
               -c

# Linker flags
LDFLAGS      = -nostdlib \
               -znocombreloc \
               -T $(GNUEFI_LDS) \
               -shared \
               -Bsymbolic \
               -L $(GNUEFI_LIB) \
               $(GNUEFI_CRT)

LIBS         = -lefi -lgnuefi

# objcopy flags to produce PE32+ .efi binary
OCFLAGS      = -j .text \
               -j .sdata \
               -j .data \
               -j .dynamic \
               -j .dynsym \
               -j .rel \
               -j .rela \
               -j .reloc \
               --target=efi-app-$(ARCH)

# ──────────────────────────────────────────────
all: $(TARGET)

%.o: %.c
	$(CC) $(CFLAGS) -o $@ $<

main.so: $(OBJS)
	$(LD) $(LDFLAGS) $^ -o $@ $(LIBS)

$(TARGET): main.so
	$(OBJCOPY) $(OCFLAGS) $< $@

# Create an ESP (FAT) disk image and boot in QEMU
run: $(TARGET)
	@mkdir -p esp/EFI/BOOT
	cp $(TARGET) esp/EFI/BOOT/BOOTX64.EFI
	qemu-system-x86_64 \
		-bios $(OVMF) \
		-drive format=raw,file=fat:rw:esp \
		-net none \
		-nographic

clean:
	rm -f $(OBJS) main.so $(TARGET)
	rm -rf esp

.PHONY: all clean run
