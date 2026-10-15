#!/usr/bin/env python3
"""vibe-web の PNG のテストデータ（ほかの実装 = PIL で作った PNG）を作る。

画素の値は式で決め、テスト（src/tests/png.rs）でも同じ式で期待する値を作る。作り直すときは
このディレクトリで python3 make-png.py。PIL はふつうの並べ方だけ（Adam7 はテストの中の符号化器で作る）。
"""
from PIL import Image


def rgb(x, y):
    return ((x * 40) % 256, (y * 50) % 256, ((x + y) * 30) % 256)


# RGB 8 ビット
im = Image.new("RGB", (13, 7))
im.putdata([rgb(x, y) for y in range(7) for x in range(13)])
im.save("RGB8.PNG")

# RGBA 8 ビット（アルファは x で変える）
im = Image.new("RGBA", (7, 6))
im.putdata([rgb(x, y) + ((x * 60) % 256,) for y in range(6) for x in range(7)])
im.save("RGBA8.PNG")

# グレー 16 ビット
im = Image.new("I;16", (5, 3))
im.putdata([(x * 13000 + y * 7000) % 65536 for y in range(3) for x in range(5)])
im.save("GRAY16.PNG")

# グレー + アルファ 8 ビット
im = Image.new("LA", (6, 4))
im.putdata([(x * 20, y * 80) for y in range(4) for x in range(6)])
im.save("GRAYA8.PNG")

# 1 ビット
im = Image.new("1", (11, 3))
im.putdata([255 if (x + y) % 3 == 0 else 0 for y in range(3) for x in range(11)])
im.save("GRAY1.PNG")

# パレット 4 ビット（16 色。0 番は透明、1 番は半分）
im = Image.new("P", (9, 5))
pal = []
for i in range(16):
    pal += [i * 16, 255 - i * 16, i * 8]
im.putpalette(pal)
im.putdata([(x + 2 * y) % 16 for y in range(5) for x in range(9)])
im.save("PAL4.PNG", bits=4, transparency=bytes([0, 128]))
