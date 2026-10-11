# VibeOS

x86 PC 向けの小さな OS。**AI（Claude）が OS をどこまで作れるかを見る実験**として開発している。

- 32bit プロテクトモード（486 以降）
- 1.44MB フロッピーイメージから起動
- 640×480・256 色（VESA）のデスクトップ、または 320×200（VGA モード 13h）。起動時に選ぶ
- ブート部は NASM、カーネル以降は Rust
- QEMU でテストしつつ、実機（レガシー BIOS の PC）でも動くことを目指す

## ドキュメント

| ファイル | 内容 |
|---|---|
| [docs/DESIGN.md](docs/DESIGN.md) | 設計：完成した姿と仕組み |
| [docs/PLAN.md](docs/PLAN.md) | 計画：Phase、完了条件、今どこにいるか |
| [docs/DECISIONS.md](docs/DECISIONS.md) | 決定の記録：何を、なぜ決めたか |
| [docs/LOG.md](docs/LOG.md) | 経過の記録：やったこと、詰まったこと |
| [CLAUDE.md](CLAUDE.md) | AI が作業中に守るルール |

## 必要なもの

- `nasm`
- `rustup`（nightly と `rust-src`。版は `rust-toolchain.toml` で固定）
- `llvm-objcopy`
- `mtools`, `dosfstools`（`mkfs.fat`）
- `qemu-system-i386`
- `make`
- （任意）`gdb`, Bochs, 86Box

## ビルドと実行

Phase 0 で用意する予定（現在は未実装）。

```bash
make          # build/vibeos.img を作る
make run      # QEMU で起動
make test     # 自動テスト
make debug    # gdb で接続できる状態で起動
```

## ライセンス

MIT（予定）
