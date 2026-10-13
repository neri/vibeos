#!/bin/sh
# Noto のフォント（TrueType）を GitHub の公式の置き場所から取ってきて、SHA-256 を確かめる。
# make fonts（assets/fonts の .VFN を作り直す）が使う。ふだんのビルドには要らない（.VFN はコミットしてある）。
#
# 使い方: tools/fetch-fonts.sh DIR
# 置き場所の版はコミットで決めてある（notofonts.github.io の 2024-11-21 のもの。DECISIONS.md D-052）。
# すでに正しいファイルがあれば取り直さない。
set -eu

DIR=${1:?usage: fetch-fonts.sh DIR}
COMMIT=28b15b4b43b7bed62b5cf6e6b0b5ff5846270535
BASE=https://raw.githubusercontent.com/notofonts/notofonts.github.io/$COMMIT/fonts

# ファイル名  置き場所（BASE から）  SHA-256
FONTS="
NotoSans-Regular.ttf NotoSans/unhinted/ttf/NotoSans-Regular.ttf f3961a9cde016d41a4879aecda1474d3a36d6bf54fa0e4643de029cc2248b0e8
NotoSans-Bold.ttf NotoSans/unhinted/ttf/NotoSans-Bold.ttf 87cb2d84472a7d66da659ee47b6cdb9552326e8c128245231f191b6ac72529d9
NotoSansMono-Regular.ttf NotoSansMono/unhinted/ttf/NotoSansMono-Regular.ttf 87f8ce0522a6c99b743ee5fc75b4073cfdd575639119672828b7b9944b65b4f4
"

mkdir -p "$DIR"
echo "$FONTS" | while read -r name path sum; do
    [ -n "$name" ] || continue
    file="$DIR/$name"
    if [ -f "$file" ] && echo "$sum  $file" | sha256sum -c --status; then
        continue
    fi
    echo "fetch-fonts: $name"
    curl -sSfL -o "$file.tmp" "$BASE/$path"
    if ! echo "$sum  $file.tmp" | sha256sum -c --status; then
        echo "fetch-fonts: $name: SHA-256 does not match" >&2
        rm -f "$file.tmp"
        exit 1
    fi
    mv "$file.tmp" "$file"
done
echo "fetch-fonts: ok ($DIR)"
