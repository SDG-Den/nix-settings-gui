#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/assets/style-catalog/previews"
mkdir -p "$out"

render_font() {
  pango-view --no-display --pixels --font="$1 18" \
    --text="Aa Bb Cc 0123  {}[]" --background="#ffffff" \
    --foreground="#111111" --margin=8 --width=400 --height=56 \
    -o "$out/$2"
}

render_font "DejaVu Sans" font-dejavu-sans.png
render_font "DejaVu Sans Mono" font-dejavu-sans-mono.png
render_font "DejaVu Serif" font-dejavu-serif.png
render_font "Fira Code" font-fira-code.png
render_font "Inter" font-inter.png
render_font "Liberation Sans" font-liberation-sans.png
render_font "Liberation Mono" font-liberation-mono.png
render_font "Liberation Serif" font-liberation-serif.png
render_font "Noto Sans CJK JP" font-noto-sans-cjk-jp.png
render_font "Noto Sans Mono CJK JP" font-noto-sans-mono-cjk-jp.png
render_font "Noto Serif CJK JP" font-noto-serif-cjk-jp.png
render_font "CaskaydiaCove Nerd Font" font-caskaydiacove-nerd-font.png
render_font "CaskaydiaCove Nerd Font Mono" font-caskaydiacove-nerd-font-mono.png
render_font "CaskaydiaMono Nerd Font" font-caskaydiamono-nerd-font.png

python3 - "$out" <<'PY'
import io
import pathlib
import struct
import subprocess
import tarfile
import sys
import tempfile
import urllib.request
import zlib

out = pathlib.Path(sys.argv[1])
base = "https://raw.githubusercontent.com/PapirusDevelopmentTeam/papirus-icon-theme/master/Papirus/24x24/"
icons = [
    "places/folder.svg",
    "apps/internet-web-browser.svg",
    "apps/system-file-manager.svg",
    "actions/edit-copy.svg",
    "status/dialog-information.svg",
    "devices/computer.svg",
    "apps/utilities-terminal.svg",
    "actions/view-refresh.svg",
]

def fetch(rel):
    rel = pathlib.PurePosixPath(rel)
    for _ in range(4):
        data = urllib.request.urlopen(base + str(rel)).read()
        text = data.decode("utf-8", "ignore").lstrip()
        if text.startswith("<"):
            return data
        target = text.strip().split("/")[-1]
        rel = rel.parent / target
    raise RuntimeError(f"cannot resolve {rel}")

with tempfile.TemporaryDirectory() as tmp:
    tmp = pathlib.Path(tmp)
    for index, rel in enumerate(icons, 1):
        svg = tmp / f"{index}.svg"
        svg.write_bytes(fetch(rel))
        png = out / f"icon-papirus-{index}-{pathlib.PurePosixPath(rel).stem}.png"
        subprocess.run(["rsvg-convert", "-w", "32", "-h", "32", "-o", str(png), str(svg)], check=True)

MAGIC = b"Xcur"
IMAGE = 0xFFFD0002
names = ["default", "pointer", "text", "wait", "crosshair", "move", "not-allowed", "ew-resize", "ns-resize"]
cursor_dir = pathlib.Path.home() / ".local/share/icons/Bibata-Modern-Classic/cursors"

def decode(data):
    if data[:4] != MAGIC:
        return None
    header, _, count = struct.unpack_from("<III", data, 4)
    if header < 16 or count <= 0 or header + count * 12 > len(data):
        return None
    best = None
    for index in range(count):
        kind, nominal, position = struct.unpack_from("<III", data, header + index * 12)
        if kind != IMAGE or position + 36 > len(data):
            continue
        chunk_header, chunk_kind, chunk_nominal, _, width, height, _, _, _ = struct.unpack_from(
            "<IIIIIIIII", data, position
        )
        if chunk_kind != IMAGE or chunk_header < 36 or width == 0 or height == 0:
            continue
        start = position + chunk_header
        if start + width * height * 4 > len(data):
            continue
        candidate = (chunk_nominal, width * height, width, height, start)
        if best is None:
            best = candidate
        elif chunk_nominal <= 48 and (best[0] > 48 or chunk_nominal > best[0]):
            best = candidate
    if best is None:
        return None
    _, _, width, height, start = best
    rgba = bytearray()
    for index in range(width * height):
        pixel = struct.unpack_from("<I", data, start + index * 4)[0]
        rgba += bytes(((pixel >> 16) & 0xFF, (pixel >> 8) & 0xFF, pixel & 0xFF, (pixel >> 24) & 0xFF))
    return width, height, bytes(rgba)

def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

def png(width, height, rgba):
    raw = bytearray()
    stride = width * 4
    for y in range(height):
        raw.append(0)
        raw += rgba[y * stride : (y + 1) * stride]
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )

def write_cursors(directory, slug):
    for index, name in enumerate(names, 1):
        path = directory / name
        if not path.is_file():
            continue
        decoded = decode(path.read_bytes())
        if decoded is None:
            continue
        width, height, rgba = decoded
        (out / f"cursor-bibata-modern-{slug}-{index}-{name}.png").write_bytes(png(width, height, rgba))

write_cursors(cursor_dir, "classic")
with tempfile.TemporaryDirectory() as cursor_tmp:
    cursor_tmp = pathlib.Path(cursor_tmp)
    for variant in ["Ice", "Amber"]:
        url = f"https://github.com/ful1e5/Bibata_Cursor/releases/download/v2.0.7/Bibata-Modern-{variant}.tar.xz"
        data = urllib.request.urlopen(url).read()
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:xz") as archive:
            archive.extractall(cursor_tmp)
        write_cursors(cursor_tmp / f"Bibata-Modern-{variant}" / "cursors", variant.lower())
PY
