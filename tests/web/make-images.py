#!/usr/bin/env python3
"""ブラウザのシナリオ（73-browse-images）の画像を作る。作り直すときはこのディレクトリで python3 make-images.py"""
from PIL import Image, ImageDraw

# LOGO.PNG: 48x32 の RGBA。横のグラデーションに丸、丸の外の右半分は透明
im = Image.new("RGBA", (48, 32))
for y in range(32):
    for x in range(48):
        a = 255 if x < 24 else 0
        im.putpixel((x, y), (x * 5, 80 + y * 4, 200, a))
d = ImageDraw.Draw(im)
d.ellipse((12, 4, 36, 28), fill=(250, 180, 40, 255))
im.save("LOGO.PNG")

# TILE.BMP: 32x24 の 24 ビット。市松模様
im = Image.new("RGB", (32, 24))
for y in range(24):
    for x in range(32):
        im.putpixel((x, y), (30, 140, 90) if (x // 8 + y // 8) % 2 == 0 else (240, 240, 230))
im.save("TILE.BMP")

# BAD.PNG: 途中で切れた PNG
with open("LOGO.PNG", "rb") as f:
    data = f.read()
with open("BAD.PNG", "wb") as f:
    f.write(data[: len(data) // 2])
