# VibeOS のビルド・実行・テスト
#
#   make          build/vibeos.img を作る
#   make run      QEMU で起動（シリアルは端末に出る）
#   make debug    QEMU を gdb 待ちで起動（別端末で gdb build/kernel.elf → target remote :1234）
#   make test     ホストの単体テスト + QEMU でのシナリオテスト
#   make clean

BUILD   := build
IMG     := $(BUILD)/vibeos.img
# テスト用イメージ: カーネルを qemu-test 機能付きでビルドしたもの（起動後にカーネル内テストを実行して終了する）
TEST_IMG := $(BUILD)/vibeos-test.img
QEMU    ?= qemu-system-i386
CARGO   ?= cargo
NASM    ?= nasm
OBJCOPY ?= llvm-objcopy

QEMU_FLAGS := -m 32M -cpu pentium2 \
	-drive file=$(IMG),format=raw,if=floppy \
	-boot a -vga std -nic none -no-reboot

KERNEL_ELF := target/i686-vibeos/release/kernel
TEST_KERNEL_ELF := target/test/i686-vibeos/release/kernel
VIBE_RUN   := target/release/vibe-run
SCENARIOS  := $(sort $(wildcard tests/scenarios/*.scn))

LOADER_MAX_BYTES := 32256   # 予約領域 LBA 1〜63

.PHONY: all run debug test test-host test-qemu clean FORCE

all: $(IMG)

$(BUILD):
	mkdir -p $@

# --- ブート部 ---------------------------------------------------------------
$(BUILD)/boot.bin: boot/boot.asm | $(BUILD)
	$(NASM) -f bin -I boot/ -o $@ $<

$(BUILD)/loader.bin: boot/loader.asm boot/bootinfo.inc | $(BUILD)
	$(NASM) -f bin -I boot/ -o $@ $<
	@size=$$(stat -c %s $@); if [ $$size -gt $(LOADER_MAX_BYTES) ]; then \
		echo "loader.bin is too big ($$size > $(LOADER_MAX_BYTES))"; rm -f $@; exit 1; fi

# --- カーネル（cargo が差分ビルドするので毎回呼ぶ） ------------------------------
$(KERNEL_ELF): FORCE
	cd kernel && $(CARGO) build --release

$(BUILD)/kernel.elf: $(KERNEL_ELF) | $(BUILD)
	cp $< $@

$(BUILD)/kernel.sys: $(KERNEL_ELF) | $(BUILD)
	$(OBJCOPY) -O binary $< $@

$(TEST_KERNEL_ELF): FORCE
	cd kernel && $(CARGO) build --release --features qemu-test --target-dir ../target/test

$(BUILD)/test/kernel.sys: $(TEST_KERNEL_ELF)
	mkdir -p $(dir $@)
	$(OBJCOPY) -O binary $< $@

# --- イメージ ---------------------------------------------------------------
# FAT12 / 1.44MB / 予約セクタ 64（LBA 1〜63 に 2nd ステージを置く）。
# ボリュームシリアルを固定して、同じ入力なら同じイメージになるようにする。
# BPB は boot.asm にも同じ値で書いてあり、boot.bin で上書きする。
# $(1) = 出力イメージ, $(2) = kernel.sys
define make-image
	rm -f $(1).tmp
	mkfs.fat -C -F 12 -R 64 -i 56494245 -n VIBEOS $(1).tmp 1440 >/dev/null
	dd if=$(BUILD)/boot.bin of=$(1).tmp conv=notrunc bs=512 count=1 status=none
	dd if=$(BUILD)/loader.bin of=$(1).tmp conv=notrunc bs=512 seek=1 status=none
	mcopy -i $(1).tmp $(2) ::KERNEL.SYS
	mcopy -s -i $(1).tmp rootfs/* ::/
	fsck.fat -n $(1).tmp >/dev/null
	mv $(1).tmp $(1)
endef

# ディスクに入れるファイル（rootfs/ 以下。8.3 形式の大文字の名前にすること）
ROOTFS := $(shell find rootfs -type f)

$(IMG): $(BUILD)/boot.bin $(BUILD)/loader.bin $(BUILD)/kernel.sys $(BUILD)/kernel.elf $(ROOTFS)
	$(call make-image,$@,$(BUILD)/kernel.sys)

$(TEST_IMG): $(BUILD)/boot.bin $(BUILD)/loader.bin $(BUILD)/test/kernel.sys $(ROOTFS)
	$(call make-image,$@,$(BUILD)/test/kernel.sys)

# --- 実行 -------------------------------------------------------------------
run: $(IMG)
	$(QEMU) $(QEMU_FLAGS) -serial stdio

debug: $(IMG)
	$(QEMU) $(QEMU_FLAGS) -serial stdio -s -S

# --- テスト -----------------------------------------------------------------
test: test-host test-qemu

test-host:
	$(CARGO) test --workspace --exclude kernel

$(VIBE_RUN): FORCE
	$(CARGO) build --release -p vibe-run

test-qemu: $(IMG) $(TEST_IMG) $(VIBE_RUN)
	$(VIBE_RUN) test --img $(IMG) --out $(BUILD)/test-out $(SCENARIOS)

clean:
	rm -rf $(BUILD)
	$(CARGO) clean
