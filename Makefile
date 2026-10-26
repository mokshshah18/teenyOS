# teenyOS build.
#   make            build build/teenyOS.bzImage (kernel + embedded root fs)
#   make run        boot it in QEMU with a VGA window
#   make run-serial boot it in QEMU on this terminal
#   make test       boot it headless and run the smoke test
#   make clean      remove build output (keeps the kernel tarball)

KVER    := 6.6.58
KSHA256 := e7df81e588d70fab5ec3ec3bb04ac53d51f0860fc3b1ec45e0a4167a026899db
KURL    := https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-$(KVER).tar.xz

# The whole system has to fit in this many bytes.
BUDGET  := 1000000
# RAM given to QEMU. See README for how low this can go.
MEM     ?= 8M

TOP     := $(CURDIR)
BUILD   := $(TOP)/build
KSRC    := $(BUILD)/linux-$(KVER)
KOUT    := $(BUILD)/kernel
TEENY   := $(TOP)/userland/target/i686-unknown-linux-musl/release/teeny
IMAGE   := $(BUILD)/teenyOS.bzImage
JOBS    ?= $(shell nproc)

.PHONY: all userland run run-serial test clean distclean FORCE

all: $(IMAGE)

userland:
	cd userland && cargo build --release

$(TEENY): userland

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
	scripts/gen_initramfs.sh $(TEENY) rootfs > $@.tmp
	cmp -s $@.tmp $@ || mv $@.tmp $@
	rm -f $@.tmp

$(KOUT)/.config: kernel/teeny.config $(KSRC)/Makefile
	mkdir -p $(KOUT)
	$(MAKE) -C $(KSRC) O=$(KOUT) ARCH=i386 tinyconfig >/dev/null
	sed 's|^CONFIG_INITRAMFS_SOURCE=.*|CONFIG_INITRAMFS_SOURCE="$(BUILD)/initramfs.list"|' kernel/teeny.config > $(KOUT)/teeny.config
	cd $(KOUT) && $(KSRC)/scripts/kconfig/merge_config.sh -m -O $(KOUT) .config teeny.config >/dev/null
	$(MAKE) -C $(KSRC) O=$(KOUT) ARCH=i386 olddefconfig >/dev/null
	scripts/check_config.sh $(KOUT)/teeny.config $(KOUT)/.config

# Always ask kbuild: it tracks the files inside initramfs.list itself.
$(IMAGE): $(KOUT)/.config $(BUILD)/initramfs.list FORCE
	$(MAKE) -C $(KSRC) O=$(KOUT) ARCH=i386 -j$(JOBS) bzImage
	cp $(KOUT)/arch/x86/boot/bzImage $@
	@size=$$(stat -c %s $@); \
	echo "teenyOS image: $$size bytes (budget $(BUDGET))"; \
	if [ $$size -gt $(BUDGET) ]; then echo "over budget"; rm -f $@; exit 1; fi

run: $(IMAGE)
	qemu-system-i386 -m $(MEM) -kernel $(IMAGE)

run-serial: $(IMAGE)
	qemu-system-i386 -m $(MEM) -kernel $(IMAGE) -nographic -append "console=ttyS0"

test: $(IMAGE)
	scripts/smoke_test.py $(IMAGE) $(MEM)

clean:
	rm -rf $(KOUT) $(BUILD)/initramfs.list $(IMAGE)
	cd userland && cargo clean

distclean: clean
	rm -rf $(BUILD)
