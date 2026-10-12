# VibeOS のビルド・実行・テスト
#
#   make          build/vibeos.img を作る
#   make run      QEMU で起動（シリアルは端末に出る）
#   make debug    QEMU を gdb 待ちで起動（別端末で gdb build/kernel.elf → target remote :1234）
#   make test     ホストの単体テスト + QEMU でのシナリオテスト
#   make bochs    Bochs で起動（画面は X のウィンドウ）
#   make test-bochs  シナリオテストを Bochs で（画面・マウス・QEMU 専用のものは飛ばす）
#   make test-wan    インターネットの決まった相手（tests/wan/ に書いたものだけ）と通信する。オーナーが許可した宛先だけ。
#                    make test には含めない
#   make test-lan    ホストの LAN の機器（LAN_HOST, 既定はデフォルトゲートウェイ）との通信を試す（make test には含めない）
#   make clean

BUILD   := build
IMG     := $(BUILD)/vibeos.img
# テスト用イメージ: カーネルを qemu-test 機能付きでビルドしたもの（起動後にカーネル内テストを実行して終了する）
TEST_IMG := $(BUILD)/vibeos-test.img
# LOADER.SYS がとびとびのクラスタに置かれたイメージ（ブートセクタが FAT をたどれるかのテスト用）
FRAG_IMG := $(BUILD)/vibeos-frag.img
QEMU    ?= qemu-system-i386
CARGO   ?= cargo
NASM    ?= nasm
OBJCOPY ?= llvm-objcopy

# ネットワーク: NE2000（I/O 0x300, IRQ 10）を slirp（QEMU の NAT）につなぐ。ゲストからホストの LAN と
# インターネットへ出られる（ping はホストの net.ipv4.ping_group_range が要る。DECISIONS.md D-032）。
# ゲストの httpd はホストの http://$(HTTP_BIND):8080/ から見える（LAN の他の機械から見せるなら
# HTTP_BIND=0.0.0.0）
HTTP_BIND ?= 127.0.0.1
QEMU_NET := -netdev user,id=net0,hostfwd=tcp:$(HTTP_BIND):8080-:80 \
	-device ne2k_isa,netdev=net0,iobase=0x300,irq=10
QEMU_FLAGS := -m 32M -cpu 486 \
	-drive file=$(IMG),format=raw,if=floppy \
	-boot a -vga std $(QEMU_NET) -rtc base=localtime -no-reboot

KERNEL_ELF := target/i486-vibeos/release/kernel
TEST_KERNEL_ELF := target/test/i486-vibeos/release/kernel
VIBE_RUN   := target/release/vibe-run
SCENARIOS  := $(sort $(wildcard tests/scenarios/*.scn))

LOADER_MAX_BYTES := 32256   # 0x8000 に読み込む。中継バッファ（0x10000）の手前まで

.PHONY: all run debug bochs test test-host test-qemu test-bochs test-lan test-wan screenshots clean FORCE

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
# 標準の FAT12 / 1.44MB（予約セクタ 1）。2nd ステージは LOADER.SYS というファイルにし、
# ルートディレクトリの先頭に置く（ブートセクタが FAT をたどって読む）。
# ボリュームシリアルを固定して、同じ入力なら同じイメージになるようにする。
# BPB は boot.asm にも同じ値で書いてあり、mkfs.fat の BPB（オフセット 11〜61）と
# 一致することを確かめてから boot.bin で上書きする。
# $(1) = 出力イメージ, $(2) = kernel.sys
define make-image
	rm -f $(1).tmp
	mkfs.fat -C -F 12 -R 1 -i 56494245 -n VIBEOS $(1).tmp 1440 >/dev/null
	@cmp -s -i 11 -n 51 $(1).tmp $(BUILD)/boot.bin || { \
		echo "BPB in boot/boot.asm differs from mkfs.fat"; rm -f $(1).tmp; exit 1; }
	dd if=$(BUILD)/boot.bin of=$(1).tmp conv=notrunc bs=512 count=1 status=none
	mcopy -i $(1).tmp $(BUILD)/loader.bin ::LOADER.SYS
	mcopy -i $(1).tmp $(2) ::KERNEL.SYS
	mcopy -s -i $(1).tmp rootfs/* ::/
	mcopy -s -i $(1).tmp $(GEN_ROOTFS)/* ::/
	mcopy -s -i $(1).tmp $(APPS_DIR)/* ::/
	fsck.fat -n $(1).tmp >/dev/null
	mv $(1).tmp $(1)
endef

# ディスクに入れるファイル（rootfs/ 以下。8.3 形式の大文字の名前にすること）
ROOTFS := $(shell find rootfs -type f)
# ユーザアプリ（apps/ の中のバイナリ）。ディスクの /BIN/名前.ELF に入れる
APPS := hello fault view paint mandel snake more fstest edit clock life ping host ntp get httpd
APPS_DIR := $(BUILD)/rootfs-apps
APPS_STAMP := $(APPS_DIR)/.stamp

$(APPS_STAMP): FORCE | $(BUILD)
	cd apps && $(CARGO) build --release $(addprefix -p ,$(APPS))
	mkdir -p $(APPS_DIR)/BIN
	@for a in $(APPS); do \
		name=$$(echo $$a | tr a-z A-Z); \
		$(OBJCOPY) --strip-all target/apps/i486-vibeos/release/$$a $(APPS_DIR)/BIN/$$name.ELF.tmp; \
		if ! cmp -s $(APPS_DIR)/BIN/$$name.ELF.tmp $(APPS_DIR)/BIN/$$name.ELF; then \
			mv $(APPS_DIR)/BIN/$$name.ELF.tmp $(APPS_DIR)/BIN/$$name.ELF; touch $@; \
		else rm $(APPS_DIR)/BIN/$$name.ELF.tmp; fi; \
	done
	@test -f $@ || touch $@

# ビルド時に生成してディスクに入れるもの（tools/mkassets）
GEN_ROOTFS := $(BUILD)/rootfs-gen
GEN_STAMP := $(GEN_ROOTFS)/.stamp

$(GEN_STAMP): $(shell find tools/mkassets -type f) | $(BUILD)
	rm -rf $(GEN_ROOTFS)
	$(CARGO) run -q --release -p mkassets -- $(GEN_ROOTFS)
	touch $@

$(IMG): $(BUILD)/boot.bin $(BUILD)/loader.bin $(BUILD)/kernel.sys $(BUILD)/kernel.elf $(ROOTFS) $(GEN_STAMP) $(APPS_STAMP)
	$(call make-image,$@,$(BUILD)/kernel.sys)

$(TEST_IMG): $(BUILD)/boot.bin $(BUILD)/loader.bin $(BUILD)/test/kernel.sys $(ROOTFS) $(GEN_STAMP) $(APPS_STAMP)
	$(call make-image,$@,$(BUILD)/test/kernel.sys)

# 先頭の LOADER.SYS を消し、空いた所に 1 クラスタのファイルを並べて 1 つおきに消してから
# 置き直す。LOADER.SYS は 1 クラスタずつの飛び地と、後ろの空き領域の連続した塊に分かれる
$(FRAG_IMG): $(IMG)
	cp $< $@.tmp
	mdel -i $@.tmp ::LOADER.SYS
	head -c 512 /dev/zero > $(BUILD)/pad.tmp
	for i in 1 2 3 4 5 6 7 8; do mcopy -i $@.tmp $(BUILD)/pad.tmp ::PAD$$i.TMP || exit 1; done
	for i in 2 4 6 8; do mdel -i $@.tmp ::PAD$$i.TMP || exit 1; done
	mcopy -i $@.tmp $(BUILD)/loader.bin ::LOADER.SYS
	rm $(BUILD)/pad.tmp
	fsck.fat -n $@.tmp >/dev/null
	mv $@.tmp $@

# --- 実行 -------------------------------------------------------------------
run: $(IMG)
	$(QEMU) $(QEMU_FLAGS) -serial stdio

debug: $(IMG)
	$(QEMU) $(QEMU_FLAGS) -serial stdio -s -S

bochs: $(IMG)
	cp $(IMG) $(BUILD)/bochs.img
	BOCHS_FLOPPY=$(BUILD)/bochs.img BOCHS_SERIAL=/dev/stdout BOCHS_LOG=$(BUILD)/bochs.log \
		BOCHS_DISPLAY=x BOCHS_DISPLAY_OPTIONS= bochs -q -f tools/bochs/bochsrc.txt -rc tools/bochs/continue.rc

# --- テスト -----------------------------------------------------------------
test: test-host test-qemu

test-host:
	$(CARGO) test --workspace --exclude kernel --exclude vibe-rt $(addprefix --exclude ,$(APPS))

$(VIBE_RUN): FORCE
	$(CARGO) build --release -p vibe-run

test-qemu: $(IMG) $(TEST_IMG) $(FRAG_IMG) $(VIBE_RUN)
	$(VIBE_RUN) test --img $(IMG) --out $(BUILD)/test-out $(SCENARIOS)

test-bochs: $(IMG) $(TEST_IMG) $(FRAG_IMG) $(VIBE_RUN)
	$(VIBE_RUN) test --bochs --img $(IMG) --out $(BUILD)/test-out-bochs $(SCENARIOS)

# LAN の機器（既定はデフォルトゲートウェイ = ルータ）に HTTP と ping でつなぐ
LAN_HOST ?= $(shell ip route 2>/dev/null | awk '/^default/ {print $$3; exit}')
LAN_SCENARIOS := $(sort $(wildcard tests/lan/*.scn))

test-lan: $(IMG) $(VIBE_RUN)
	@test -n "$(LAN_HOST)" || { echo "set LAN_HOST=address of a LAN device with an HTTP server"; exit 1; }
	@g=$$(id -g); set -- $$(cat /proc/sys/net/ipv4/ping_group_range); \
		if [ $$g -lt $$1 ] || [ $$g -gt $$2 ]; then \
		echo "note: net.ipv4.ping_group_range ($$1 $$2) does not include your group $$g; 02-lan-ping will fail"; fi
	LAN_HOST=$(LAN_HOST) $(VIBE_RUN) test --img $(IMG) --out $(BUILD)/test-out-lan $(LAN_SCENARIOS)

# インターネットの相手と通信する（DECISIONS.md D-030。宛先は tests/wan/ のシナリオに書いたものだけ。
# 宛先を増やすときはオーナーの許可を取る）
test-wan: $(IMG) $(VIBE_RUN)
	$(VIBE_RUN) test --img $(IMG) --out $(BUILD)/test-out-wan $(sort $(wildcard tests/wan/*.scn))

# --- スクリーンショット（README 用。docs/images/ に置く）----------------------
screenshots: $(IMG) $(VIBE_RUN)
	$(VIBE_RUN) test --img $(IMG) --out $(BUILD)/screenshots tools/screenshots/*.scn
	mkdir -p docs/images
	cp $(BUILD)/screenshots/desktop.png $(BUILD)/screenshots/shell-320.png $(BUILD)/screenshots/editor-320.png docs/images/

clean:
	rm -rf $(BUILD)
	$(CARGO) clean
