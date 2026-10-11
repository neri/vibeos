# 経過の記録

作業したこと・詰まったこと・分かったことを日付順に残す。新しいものを下に追加する。

---

## 追加した外部クレート

| 日付 | クレート | バージョン | 用途 | 理由 |
|---|---|---|---|---|
| 2026-10-11 | serde_json（ホスト） | 1 | vibe-run の QMP 通信 | ホスト側ツール |
| 2026-10-11 | png（ホスト） | 0.17 | vibe-run の画面比較 | ホスト側ツール |

---

## 2026-10-11 設計

### 決めたこと
設計の議論を行い、DESIGN.md / PLAN.md / DECISIONS.md を作成した。決定の経緯は DECISIONS.md D-001〜D-017。流れ:
1. オーナーが前提（x86, フロッピーイメージ, QEMU, モード 13h）を提示 → AI が設計書ドラフトを作成（言語は C を推奨）
2. 実機を想定しない場合に何が変わるかを AI が説明
3. オーナーが方針を決定: 目的は「AI がどこまで作れるか」、言語は Rust、QEMU 専用にしない、日本語は見送り、キーボードは US
4. オーナーが 32bit と外部クレートの方針を決定
5. オーナーがマルチタスクの後付けを懸念 → スケジューラを Phase 5 に前倒し
6. オーナーの問いかけで、設計書を設計・計画・決定・経過に分割

### 環境の確認
- あり: `nasm`, `qemu-system-i386`（11.1.2, `~/opt/qemu-11.1.2`）, `mtools`, `mkfs.fat`, `clang`, `gcc`, `ld.lld`, `llvm-objcopy`, `gdb`, `make`
- Rust: stable 1.99.0, nightly 1.101.0（2026-10-09）, nightly に `rust-src` 導入済み
- 無し: `i686-elf-gcc`, `zig`, Bochs, 86Box
- `rustc --print target-list` に `i686-unknown-none` は無い（`x86_64-unknown-none` と `i686-unknown-uefi` はある）
- QEMU は `-cpu pentium2` に対応

### Rust の 32bit ベアメタル試作
スクラッチ領域で最小のカーネル（`_start` から `kmain` を呼び VRAM に書き込むだけ）をビルドした。
- `rustc +nightly -Z unstable-options --print target-spec-json --target i686-unknown-linux-gnu` の出力を元にカスタムターゲット JSON を作成
- **詰まった点 1**: `"rustc-abi": "x86-softfloat"` は拒否された。この nightly で使える値は `softfloat` / `x86-sse2` / `powerpc-spe` / `sparc-v8plus`。`softfloat` で通った → nightly の版を固定する理由になった
- ビルドは `-Zbuild-std=core,alloc,compiler_builtins -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec` で成功（約 9 秒）。`cpu` は `pentium4` と `i686` のどちらでも通った
- 出力は `ELF 32-bit LSB executable, Intel 80386`。逆アセンブルで VRAM（`0xa0000`）への書き込みループを確認
- **詰まった点 2**: `.eh_frame_hdr` / `.eh_frame` が `0x100000` に置かれ、`_start` が `0x100030` にずれた → リンカスクリプトで `.text.entry` を先頭に置き `.eh_frame*` を捨てる必要がある
- QEMU での起動は未確認（ローダがまだ無いため）

## 2026-10-11 Phase 0: 環境構築

- `git init`（ブランチ `main`）。`.gitignore` は `build/` と `target/`
- nightly を `nightly-2026-10-10`（rustc 1.101.0-nightly 32dba69d6）に固定し `rust-toolchain.toml` に記載。rustup で同じ日付の版を追加でインストールした
- Cargo ワークスペース: `kernel` と `tools/vibe-run`。ルートの `cargo build` / `cargo test` はホスト向けだけ（`default-members`）。カーネルは `kernel/` に入ってビルドし、`kernel/.cargo/config.toml` でターゲットと build-std を指定する
- リンカスクリプトは `kernel/build.rs` から絶対パスで渡す（`-C link-arg=-T相対パス` は rustc の作業ディレクトリ次第で壊れるため）
- カーネルの空実装をビルドし、エントリが `0x100000` に来ることを確認（`.eh_frame` 等は `/DISCARD/`）
- ビルド時に core の SSE 関数について「将来エラーになる」警告が出る（soft-float ターゲットで `#[target_feature(enable = "sse")]` があるため）。カーネルからは使わないので当面無視
- テストランナー `tools/vibe-run` を作成: QEMU を画面なしで起動し、QMP（UNIX ソケット）で画面取得・キー入力、シリアル出力の待ち合わせ、終了コードの確認をシナリオファイル（`tests/scenarios/*.scn`）で行う。イメージは `snapshot=on` で開くのでテストで書き換わらない
- QEMU に `-nic none` を付けた（iPXE の ROM が読まれて起動が遅くなるのを防ぐ）
- `make test` で Phase 0 のシナリオ（QEMU が起動し QMP で操作できる）が通った

