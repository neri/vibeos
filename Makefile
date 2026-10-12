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
#   make test-lzsa   自作の LZSA で、カーネル・アプリとテスト用のデータを圧縮 → 展開して元と比べる（make test に含む）
#   make test-lzsa-ref  自作の LZSA を本家の lzsa と突き合わせる（本家で展開・本家の出力を展開。make test には含めない）
#   make lzsa-bench  自作の LZSA と本家の圧縮後の大きさの表
#   make lzsa-ref    本家の lzsa を GitHub から取ってきてビルドする（build/ext/lzsa。DECISIONS.md D-033）
#   make size-report KERNEL.SYS の圧縮前後の大きさとフロッピーの空き
#   make COMPRESS=0  カーネルとアプリを圧縮しないイメージを作る（ローダは印の無い KERNEL.SYS を
#                    そのまま写し、カーネルは gzip でない ELF をそのまま読む）
#   make clean

BUILD   := build
IMG     := $(BUILD)/vibeos.img
# テスト用イメージ: カーネルを qemu-test 機能付きでビルドしたもの（起動後にカーネル内テストを実行して終了する）
TEST_IMG := $(BUILD)/vibeos-test.img
# LOADER.SYS がとびとびのクラスタに置かれたイメージ（ブートセクタが FAT をたどれるかのテスト用）
FRAG_IMG := $(BUILD)/vibeos-frag.img
# カーネルを圧縮しないイメージ（ローダが圧縮していない KERNEL.SYS も読めるかのテスト用）
RAW_IMG := $(BUILD)/vibeos-raw.img
# 圧縮したカーネルの中身を壊したイメージ（ローダが CRC32 などで気づいて止まるかのテスト用）
BROKEN_IMG := $(BUILD)/vibeos-broken.img
# 1 ならカーネルを LZSA2 + BCJ で圧縮して KERNEL.SYS にし（docs/plans/compress.md）、
# アプリを gzip で圧縮して /BIN/*.ELF にする（docs/plans/app-compress.md）
COMPRESS ?= 1
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
VIBE_LZSA  := target/release/vibe-lzsa
SCENARIOS  := $(sort $(wildcard tests/scenarios/*.scn))

LOADER_MAX_BYTES := 32256   # 0x8000 に読み込む。中継バッファ（0x10000）の手前まで

.PHONY: all run debug bochs test test-host test-qemu test-bochs test-lan test-wan test-lzsa test-lzsa-ref test-apps-gz lzsa-bench lzsa-ref size-report screenshots clean FORCE

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

# 圧縮したカーネル（"VPK1" のヘッダ + LZSA2。BCJ の基準はカーネルが置かれる 0x100000）
%.vpk: %.sys $(VIBE_LZSA)
	$(VIBE_LZSA) pack --bcj 0x100000 $< $@

# 前と違う COMPRESS で make したときにイメージを作り直すため、値を覚えておく
COMPRESS_STAMP := $(BUILD)/compress.stamp
$(COMPRESS_STAMP): FORCE | $(BUILD)
	@echo $(COMPRESS) | cmp -s - $@ || echo $(COMPRESS) > $@

# イメージに KERNEL.SYS として入れるファイル
ifeq ($(COMPRESS),1)
KERNEL_FILE := $(BUILD)/kernel.vpk
TEST_KERNEL_FILE := $(BUILD)/test/kernel.vpk
else
KERNEL_FILE := $(BUILD)/kernel.sys
TEST_KERNEL_FILE := $(BUILD)/test/kernel.sys
endif

# --- イメージ ---------------------------------------------------------------
# 標準の FAT12 / 1.44MB（予約セクタ 1）。2nd ステージは LOADER.SYS というファイルにし、
# ルートディレクトリの先頭に置く（ブートセクタが FAT をたどって読む）。
# ボリュームシリアルを固定して、同じ入力なら同じイメージになるようにする。
# BPB は boot.asm にも同じ値で書いてあり、mkfs.fat の BPB（オフセット 11〜61）と
# 一致することを確かめてから boot.bin で上書きする。
# $(1) = 出力イメージ, $(2) = kernel.sys, $(3) = アプリのディレクトリ（BIN/*.ELF）
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
	mcopy -s -i $(1).tmp $(3)/* ::/
	fsck.fat -n $(1).tmp >/dev/null
	mv $(1).tmp $(1)
endef

# ディスクに入れるファイル（rootfs/ 以下。8.3 形式の大文字の名前にすること）
ROOTFS := $(shell find rootfs -type f)
# ユーザアプリ（apps/ の中のバイナリ）。ディスクの /BIN/名前.ELF に入れる
APPS := hello fault view paint mandel snake more fstest edit clock life ping host ntp get httpd calc files
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

# gzip で圧縮したアプリ（名前は .ELF のまま。カーネルが先頭の 1F 8B で見分けて展開する）。
# -n: 名前と時刻を入れない（同じ入力なら同じイメージになるように）
APPS_GZ_DIR := $(BUILD)/rootfs-apps-gz
APPS_GZ_STAMP := $(APPS_GZ_DIR)/.stamp

$(APPS_GZ_STAMP): $(APPS_STAMP)
	rm -rf $(APPS_GZ_DIR)
	mkdir -p $(APPS_GZ_DIR)/BIN
	for f in $(APPS_DIR)/BIN/*.ELF; do gzip -9 -n -c $$f > $(APPS_GZ_DIR)/BIN/$$(basename $$f) || exit 1; done
	touch $@

ifeq ($(COMPRESS),1)
APPS_IMG_DIR := $(APPS_GZ_DIR)
APPS_IMG_STAMP := $(APPS_GZ_STAMP)
else
APPS_IMG_DIR := $(APPS_DIR)
APPS_IMG_STAMP := $(APPS_STAMP)
endif

# ビルド時に生成してディスクに入れるもの（tools/mkassets）
GEN_ROOTFS := $(BUILD)/rootfs-gen
GEN_STAMP := $(GEN_ROOTFS)/.stamp

$(GEN_STAMP): $(shell find tools/mkassets -type f) | $(BUILD)
	rm -rf $(GEN_ROOTFS)
	$(CARGO) run -q --release -p mkassets -- $(GEN_ROOTFS)
	touch $@

$(IMG): $(BUILD)/boot.bin $(BUILD)/loader.bin $(KERNEL_FILE) $(COMPRESS_STAMP) $(BUILD)/kernel.elf $(ROOTFS) $(GEN_STAMP) $(APPS_IMG_STAMP)
	$(call make-image,$@,$(KERNEL_FILE),$(APPS_IMG_DIR))

$(TEST_IMG): $(BUILD)/boot.bin $(BUILD)/loader.bin $(TEST_KERNEL_FILE) $(COMPRESS_STAMP) $(ROOTFS) $(GEN_STAMP) $(APPS_IMG_STAMP)
	$(call make-image,$@,$(TEST_KERNEL_FILE),$(APPS_IMG_DIR))

# カーネルもアプリも圧縮しない（COMPRESS=0 と同じ中身）
$(RAW_IMG): $(BUILD)/boot.bin $(BUILD)/loader.bin $(BUILD)/kernel.sys $(ROOTFS) $(GEN_STAMP) $(APPS_STAMP)
	$(call make-image,$@,$(BUILD)/kernel.sys,$(APPS_DIR))

# 圧縮したデータの途中（先頭から 40000 バイト目）を書き換える。元と変わったことも確かめる
$(BUILD)/kernel-broken.vpk: $(BUILD)/kernel.vpk
	cp $< $@
	printf '\245\132\245\132' | dd of=$@ bs=1 seek=40000 conv=notrunc status=none
	! cmp -s $< $@

$(BROKEN_IMG): $(BUILD)/boot.bin $(BUILD)/loader.bin $(BUILD)/kernel-broken.vpk $(ROOTFS) $(GEN_STAMP) $(APPS_IMG_STAMP)
	$(call make-image,$@,$(BUILD)/kernel-broken.vpk,$(APPS_IMG_DIR))

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
test: test-host test-qemu test-lzsa test-apps-gz

test-host:
	$(CARGO) test --workspace --exclude kernel --exclude vibe-rt $(addprefix --exclude ,$(APPS))

$(VIBE_RUN): FORCE
	$(CARGO) build --release -p vibe-run

test-qemu: $(IMG) $(TEST_IMG) $(FRAG_IMG) $(RAW_IMG) $(BROKEN_IMG) $(VIBE_RUN)
	$(VIBE_RUN) test --img $(IMG) --out $(BUILD)/test-out $(SCENARIOS)

test-bochs: $(IMG) $(TEST_IMG) $(FRAG_IMG) $(RAW_IMG) $(BROKEN_IMG) $(VIBE_RUN)
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

# --- LZSA（docs/plans/lzsa.md）--------------------------------------------------
# 本家の lzsa（v1.4.1）。自作の圧縮器の出力を本家で展開できるか・率がどれだけ違うかを確かめるのに使う。
# make test は本家に頼らない（自作の圧縮 → 自作の展開だけ。DECISIONS.md D-033）
LZSA_REF_COMMIT := 15ee2dfe118eeb8f7683ca44f64821c3a61ca1e5
LZSA_REF := $(BUILD)/ext/lzsa/lzsa
# 圧縮して比べる実物: カーネルとアプリ（make のたびに中身が変わる）
LZSA_FILES = $(BUILD)/kernel.sys $(sort $(wildcard $(BUILD)/rootfs-apps/BIN/*.ELF))

$(LZSA_REF):
	rm -rf $(BUILD)/ext/lzsa
	git clone --quiet https://github.com/emmanuel-marty/lzsa.git $(BUILD)/ext/lzsa
	git -C $(BUILD)/ext/lzsa checkout --quiet $(LZSA_REF_COMMIT)
	$(MAKE) -C $(BUILD)/ext/lzsa

lzsa-ref: $(LZSA_REF)

$(VIBE_LZSA): FORCE
	$(CARGO) build --release -p vibe-lzsa-tool

test-lzsa: $(IMG) $(BUILD)/kernel.vpk $(VIBE_LZSA)
	$(VIBE_LZSA) roundtrip $(LZSA_FILES)
	$(VIBE_LZSA) unpack $(BUILD)/kernel.vpk $(BUILD)/kernel-unpacked.sys
	cmp $(BUILD)/kernel-unpacked.sys $(BUILD)/kernel.sys
	@size=$$(stat -c %s $(BUILD)/kernel.vpk); orig=$$(stat -c %s $(BUILD)/kernel.sys); \
		if [ $$((size * 2)) -gt $$orig ]; then echo "kernel.vpk is not under 50% ($$size / $$orig)"; exit 1; fi

test-lzsa-ref: $(IMG) $(LZSA_REF) $(VIBE_LZSA)
	$(VIBE_LZSA) compat --ref $(LZSA_REF) --work $(BUILD)/lzsa-work $(LZSA_FILES)

# イメージの /BIN/*.ELF がすべて gzip で、展開すると元の ELF と一致する（ホストの gzip との互換）。
# Phase 30 のときのアプリ（APPS_A1）の合計が元の 45% 以下（docs/plans/app-compress.md A1）、
# 全部のアプリの合計が 50% 以下（コードの多いアプリは縮みにくい。DECISIONS.md D-037）。COMPRESS=0 のときは何もしない
APPS_A1 := hello fault view paint mandel snake more fstest edit clock life ping host ntp get httpd

test-apps-gz: $(IMG)
ifeq ($(COMPRESS),1)
	@orig=0; packed=0; orig_a1=0; packed_a1=0; for f in $(APPS_DIR)/BIN/*.ELF; do \
		n=$$(basename $$f); \
		mcopy -n -i $(IMG) ::BIN/$$n $(BUILD)/app.gz.tmp || exit 1; \
		zcat $(BUILD)/app.gz.tmp | cmp - $$f || { echo "BIN/$$n: zcat differs"; exit 1; }; \
		o=$$(stat -c %s $$f); p=$$(stat -c %s $(BUILD)/app.gz.tmp); \
		orig=$$((orig + o)); packed=$$((packed + p)); \
		for a in $(APPS_A1); do if [ "$$n" = "$$(echo $$a | tr a-z A-Z).ELF" ]; then \
			orig_a1=$$((orig_a1 + o)); packed_a1=$$((packed_a1 + p)); fi; done; \
	done; rm -f $(BUILD)/app.gz.tmp; \
	echo "apps (Phase 30): $$orig_a1 -> $$packed_a1 bytes"; \
	echo "apps (all): $$orig -> $$packed bytes"; \
	if [ $$((packed_a1 * 100)) -gt $$((orig_a1 * 45)) ]; then echo "Phase 30 apps are not under 45%"; exit 1; fi; \
	if [ $$((packed * 100)) -gt $$((orig * 50)) ]; then echo "apps are not under 50%"; exit 1; fi
endif

size-report: $(IMG) $(BUILD)/kernel.vpk $(APPS_GZ_STAMP)
	@orig=$$(stat -c %s $(BUILD)/kernel.sys); packed=$$(stat -c %s $(BUILD)/kernel.vpk); \
		echo "| kernel.sys | $$orig |"; \
		echo "| kernel.vpk (LZSA2 + BCJ) | $$packed | $$((packed * 1000 / orig / 10)).$$((packed * 1000 / orig % 10))% |"; \
		orig=$$(cat $(APPS_DIR)/BIN/*.ELF | wc -c); packed=$$(cat $(APPS_GZ_DIR)/BIN/*.ELF | wc -c); \
		echo "| apps /BIN/*.ELF | $$orig |"; \
		echo "| apps gzip -9 | $$packed | $$((packed * 1000 / orig / 10)).$$((packed * 1000 / orig % 10))% |"; \
		echo "| free on $(IMG) | $$(mdir -i $(IMG) :: | sed -n 's/ *\([0-9 ]*\) bytes free/\1/p' | tr -d ' ') |"

lzsa-bench: $(IMG) $(LZSA_REF) $(VIBE_LZSA)
	$(VIBE_LZSA) bench --ref $(LZSA_REF) --work $(BUILD)/lzsa-work $(LZSA_FILES)

# --- スクリーンショット（README 用。docs/images/ に置く）----------------------
screenshots: $(IMG) $(VIBE_RUN)
	$(VIBE_RUN) test --img $(IMG) --out $(BUILD)/screenshots tools/screenshots/*.scn
	mkdir -p docs/images
	cp $(BUILD)/screenshots/desktop.png $(BUILD)/screenshots/shell-320.png $(BUILD)/screenshots/editor-320.png docs/images/

clean:
	rm -rf $(BUILD)
	$(CARGO) clean
