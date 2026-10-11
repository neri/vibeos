# VibeOS のビルド・実行・テスト
#
#   make          build/vibeos.img を作る
#   make run      QEMU で起動（シリアルは端末に出る）
#   make debug    QEMU を gdb 待ちで起動（別端末で gdb build/kernel.elf → target remote :1234）
#   make test     ホストの単体テスト + QEMU でのシナリオテスト
#   make clean

BUILD   := build
IMG     := $(BUILD)/vibeos.img
QEMU    ?= qemu-system-i386
CARGO   ?= cargo

QEMU_FLAGS := -m 32M -cpu pentium2 \
	-drive file=$(IMG),format=raw,if=floppy \
	-boot a -vga std -nic none -no-reboot

VIBE_RUN := target/release/vibe-run
SCENARIOS := $(sort $(wildcard tests/scenarios/*.scn))

.PHONY: all run debug test test-host test-qemu clean FORCE

all: $(IMG)

$(BUILD):
	mkdir -p $@

# --- イメージ ---------------------------------------------------------------
# FAT12 / 1.44MB / 予約セクタ 64（LBA 1〜63 に 2nd ステージを置く）。
# ボリュームシリアルを固定して、同じ入力なら同じイメージになるようにする。
$(IMG): | $(BUILD)
	rm -f $@.tmp
	mkfs.fat -C -F 12 -R 64 -i 56494245 -n VIBEOS $@.tmp 1440 >/dev/null
	mv $@.tmp $@

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

test-qemu: $(IMG) $(VIBE_RUN)
	$(VIBE_RUN) test --img $(IMG) --out $(BUILD)/test-out $(SCENARIOS)

clean:
	rm -rf $(BUILD)
	$(CARGO) clean
