#!/usr/bin/env bash
# 从 UTF-8 源重生成 GBK 页面字节（hex）。改了 gbk-page.src.html 之后必须跑一次，
# 否则 /gbk 仍是旧字节（症状：页面内容与源不一致，但仍然是正确的 GBK 解码）。
#
# 依赖：iconv 与 xxd（Git Bash / Linux / macOS 自带）。
set -euo pipefail
dir="$(cd "$(dirname "$0")" && pwd)"
src="$dir/pages/batch1/k3/gbk-page.src.html"
out="$dir/pages/batch1/k3/gbk-page.hex"

iconv -f UTF-8 -t GBK "$src" | xxd -p | tr -d '\n' > "$out"
printf '\n' >> "$out"
echo "regenerated: $out"
