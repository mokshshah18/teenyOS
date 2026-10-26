# teenyOS build.
#   make             kernel for real PCs + the USB stick image
#   make mini        the sub-1MB build (console only, RAM only)
#   make core        the kernel-less UEFI edition + its bootable image
#   make run         boot the USB image in QEMU, legacy BIOS
#   make run-uefi    boot the USB image in QEMU, UEFI firmware
#   make run-mini    boot the mini build in QEMU on this terminal
#   make run-core    boot the core edition in QEMU, UEFI firmware
#   make test        automated tests in QEMU (see scripts/)
#   make core-test   end-to-end test of the core shell on real firmware
#   make clean       remove build output (keeps the kernel tarball)

KVER    := 6.6.58
KSHA256 := e7df81e588d70fab5ec3ec3bb04ac53d51f0860fc3b1ec45e0a4167a026899db
KURL    := https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-$(KVER).tar.xz

TOP     := $(CURDIR)
BUILD   := $(TOP)/build
KSRC    := $(BUILD)/linux-$(KVER)
TEENY   := $(TOP)/userland/target/x86_64-unknown-linux-musl/release/teeny
JOBS    ?= $(shell nproc)
MEM     ?= 64M

# Each flavor is a list of config layers and a size budget in bytes.
LAYERS_mini := kernel/base.config
LAYERS_full := kernel/base.config kernel/full.config
BUDGET_mini := 1000000
BUDGET_full := 2200000

KERNEL_full := $(BUILD)/teenyOS.efi
KERNEL_mini := $(BUILD)/teenyOS-mini.bzImage
USB_IMAGE   := $(BUILD)/teenyOS-usb.img
CORE_EFI    := $(TOP)/core/target/x86_64-unknown-uefi/release/teenycore.efi
CORE_IMAGE  := $(BUILD)/teenyOS-core.img
BUDGET_core := 1000000
LOADER      := $(BUILD)/loader.bin
OVMF        := /usr/share/OVMF/OVMF_CODE_4M.fd
OVMF_VARS   := /usr/share/OVMF/OVMF_VARS_4M.fd

.DELETE_ON_ERROR:
.PHONY: all mini core userland run run-uefi run-mini run-core test core-test clean distclean FORCE

all: $(KERNEL_full) $(USB_IMAGE)
mini: $(KERNEL_mini)
core: $(CORE_IMAGE)

userland:
	cd userland && cargo build --release

$(BUILD)/linux-$(KVER).tar.xz:
	mkdir -p $(BUILD)
	curl -fL -o $@.part $(KURL)
	echo "$(KSHA256)  $@.part" | sha256sum -c -
	mv $@.part $@

$(KSRC)/Makefile: $(BUILD)/linux-$(KVER).tar.xz $(wildcard kernel/patches/*.patch)
	rm -rf $(KSRC)
	tar -C $(BUILD) -xf $<
	for p in $(sort $(wildcard kernel/patches/*.patch)); do patch -d $(KSRC) -p1 < $$p || exit 1; done
	touch $@

$(BUILD)/initramfs.list: userland scripts/gen_initramfs.sh $(shell find rootfs)
	mkdir -p $(BUILD)
	scripts/gen_initramfs.sh $(TEENY) rootfs > $@.tmp
	cmp -s $@.tmp $@ || mv $@.tmp $@
	rm -f $@.tmp

# $(call kernel,flavor): configure and build one kernel flavor.
define kernel
$(BUILD)/kernel-$(1)/.config: $(LAYERS_$(1)) $(KSRC)/Makefile
	mkdir -p $(BUILD)/kernel-$(1)
	$(MAKE) -C $(KSRC) O=$(BUILD)/kernel-$(1) tinyconfig >/dev/null
	cat $(LAYERS_$(1)) | sed 's|^CONFIG_INITRAMFS_SOURCE=.*|CONFIG_INITRAMFS_SOURCE="$(BUILD)/initramfs.list"|' > $(BUILD)/kernel-$(1)/teeny.config
	cd $(BUILD)/kernel-$(1) && $(KSRC)/scripts/kconfig/merge_config.sh -m -O . .config teeny.config >/dev/null
	$(MAKE) -C $(KSRC) O=$(BUILD)/kernel-$(1) olddefconfig >/dev/null
	scripts/check_config.sh $(BUILD)/kernel-$(1)/teeny.config $(BUILD)/kernel-$(1)/.config

# Always ask kbuild: it tracks the files inside initramfs.list itself.
$(KERNEL_$(1)): $(BUILD)/kernel-$(1)/.config $(BUILD)/initramfs.list FORCE
	$(MAKE) -C $(KSRC) O=$(BUILD)/kernel-$(1) -j$(JOBS) bzImage
	cp $(BUILD)/kernel-$(1)/arch/x86/boot/bzImage $$@
	@size=$$$$(stat -c %s $$@); \
	echo "teenyOS $(1) kernel: $$$$size bytes (budget $(BUDGET_$(1)))"; \
	if [ $$$$size -gt $(BUDGET_$(1)) ]; then echo "over budget"; rm -f $$@; exit 1; fi
endef
$(eval $(call kernel,full))
$(eval $(call kernel,mini))

$(LOADER): boot/loader.S
	mkdir -p $(BUILD)
	as --32 -o $(BUILD)/loader.o $<
	ld -m elf_i386 -Ttext 0x7c00 --oformat binary -o $@ $(BUILD)/loader.o

$(USB_IMAGE): $(KERNEL_full) $(LOADER) scripts/mkimage.py
	python3 scripts/mkimage.py --loader $(LOADER) --kernel $(KERNEL_full) -o $@
	@echo "USB image: $@ ($$(stat -c %s $@) bytes; write it to a stick with dd, Rufus or Etcher)"

# teenyOS core: the kernel-less UEFI edition. cargo tracks its own sources, so
# ask it every time and enforce the size budget on the result.
$(CORE_EFI): FORCE
	cd core && cargo build --release
	@size=$$(stat -c %s $@); \
	echo "teenyOS core: $$size bytes (budget $(BUDGET_core))"; \
	if [ $$size -gt $(BUDGET_core) ]; then echo "over budget"; exit 1; fi

$(CORE_IMAGE): $(CORE_EFI) scripts/mkcore.py
	mkdir -p $(BUILD)
	python3 scripts/mkcore.py $(CORE_EFI) $@

run: $(USB_IMAGE)
	qemu-system-x86_64 -m $(MEM) -drive file=$(USB_IMAGE),format=raw

run-uefi: $(USB_IMAGE)
	cp $(OVMF_VARS) $(BUILD)/ovmf_vars.fd
	qemu-system-x86_64 -m $(MEM) -drive if=pflash,format=raw,readonly=on,file=$(OVMF) \
		-drive if=pflash,format=raw,file=$(BUILD)/ovmf_vars.fd -drive file=$(USB_IMAGE),format=raw

run-mini: $(KERNEL_mini)
	qemu-system-x86_64 -m 16M -kernel $(KERNEL_mini) -nographic -append "console=ttyS0"

run-core: $(CORE_IMAGE)
	cp $(OVMF_VARS) $(BUILD)/ovmf_vars_core.fd
	qemu-system-x86_64 -m 128M -drive if=pflash,format=raw,readonly=on,file=$(OVMF) \
		-drive if=pflash,format=raw,file=$(BUILD)/ovmf_vars_core.fd -drive file=$(CORE_IMAGE),format=raw

test: $(KERNEL_mini) $(USB_IMAGE)
	scripts/smoke_test.py $(KERNEL_mini) 16M
	scripts/hw_test.py $(USB_IMAGE)

core-test:
	scripts/core_shell_test.py

clean:
	rm -rf $(BUILD)/kernel-* $(BUILD)/initramfs.list $(BUILD)/*.img $(BUILD)/*.efi $(BUILD)/*.bzImage $(BUILD)/loader.* $(BUILD)/ovmf_vars*.fd
	cd userland && cargo clean
	cd core && cargo clean

distclean: clean
	rm -rf $(BUILD)
