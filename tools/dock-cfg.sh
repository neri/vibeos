#!/bin/sh
# DOCK.CFG（ドックに並べるアプリ）を作る。アイコン（ICON_DIR/APP.svg）が無いアプリがあれば止める
# （ドックの項目はアイコンが定義されたものにする。DECISIONS.md D-058）
#
# 使い方: tools/dock-cfg.sh OUT ICON_DIR APP...
set -eu
out=$1
dir=$2
shift 2
missing=
for app in "$@"; do
	[ -f "$dir/$app.svg" ] || missing="$missing $app"
done
if [ -n "$missing" ]; then
	echo "dock-cfg: no icon in $dir for:$missing" >&2
	exit 1
fi
{
	echo "# Dock items: one app name per line (terminal = a terminal). Text after # is a comment."
	for app in "$@"; do echo "$app"; done
} > "$out.tmp"
mv "$out.tmp" "$out"
