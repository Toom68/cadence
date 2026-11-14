#!/usr/bin/env python3
"""Generate Cadence's native app icons (assets/icon.png, icon.icns, icon.ico).

Rasterizes Cadence's brand mark — a dark rounded square with a pale-lime
waveform — so every platform's icon matches. The generated files are committed;
this script is only needed to regenerate them.

Requirements: Python 3.9+ and numpy (`pip3 install numpy`). Nothing else.
Usage:      python3 scripts/generate-icons.py
"""
import struct
import sys
import zlib
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / 'assets'
RENDER = 2048  # working canvas; downsampled for every output size

BG = (0x17, 0x1C, 0x1E)
LIME = (0xD4, 0xEF, 0x91)
# Rounded rectangles in the icon's 128x128 viewBox space: (x, y, w, h, radius).
SHAPES = [
    (0, 0, 128, 128, 30, BG),       # background tile
    (25, 53, 13, 25, 6.5, LIME),
    (47, 33, 13, 65, 6.5, LIME),
    (69, 44, 13, 43, 6.5, LIME),
    (91, 57, 13, 17, 6.5, LIME),
]


def rounded_rect_sdf(x, y, cx, cy, hw, hh, r):
    """Signed distance to a rounded rectangle centered at (cx, cy)."""
    qx = np.abs(x - cx) - (hw - r)
    qy = np.abs(y - cy) - (hh - r)
    outside = np.hypot(np.maximum(qx, 0.0), np.maximum(qy, 0.0))
    inside = np.minimum(np.maximum(qx, qy), 0.0)
    return outside + inside - r


def render(size):
    """Rasterize the icon at `size` pixels with 2x supersampling."""
    s = size * 2
    scale = s / 128.0
    xs = (np.arange(s) + 0.5).reshape(1, s)
    ys = (np.arange(s) + 0.5).reshape(s, 1)
    rgb = np.zeros((s, s, 3), dtype=np.float64)
    alpha = np.zeros((s, s), dtype=np.float64)
    for x, y, w, h, r, color in SHAPES:
        d = rounded_rect_sdf(
            xs, ys,
            (x + w / 2) * scale, (y + h / 2) * scale,
            w / 2 * scale, h / 2 * scale, r * scale)
        a = np.clip(0.5 - d, 0.0, 1.0)
        keep = alpha * (1.0 - a)
        rgb = (rgb * keep[..., None]) + (np.array(color) * (a * (1.0 - keep))[..., None])
        alpha = a + keep
    # Downsample by 2 with a premultiplied-alpha box filter.
    premult = rgb * alpha[..., None]
    premult = premult.reshape(size, 2, size, 2, 3).mean(axis=(1, 3))
    alpha = alpha.reshape(size, 2, size, 2).mean(axis=(1, 3))
    safe = np.where(alpha > 0, alpha, 1.0)
    rgb = premult / safe[..., None]
    return np.dstack([np.clip(rgb, 0, 255), alpha * 255.0]).astype(np.uint8)


def png_bytes(rgba):
    """Encode an RGBA array as a PNG using only zlib + struct."""
    height, width = rgba.shape[:2]
    raw = b''.join(b'\x00' + rgba[row].tobytes() for row in range(height))
    def chunk(tag, data):
        return struct.pack('>I', len(data)) + tag + data + struct.pack('>I', zlib.crc32(tag + data))
    return (b'\x89PNG\r\n\x1a\n'
            + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 6, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(raw, 9))
            + chunk(b'IEND', b''))


def write_ico(pngs, path):
    """ICO container holding PNG-compressed frames (supported since Windows Vista)."""
    header = struct.pack('<HHH', 0, 1, len(pngs))
    offset = 6 + 16 * len(pngs)
    entries = b''
    for size, data in pngs:
        entries += struct.pack('<BBBBHHII', size if size < 256 else 0, size if size < 256 else 0,
                               0, 0, 1, 32, len(data), offset)
        offset += len(data)
    path.write_bytes(header + entries + b''.join(data for _, data in pngs))


# ICNS icon types -> pixel size of the embedded PNG.
ICNS_ENTRIES = [
    ('ic11', 32),   # 16x16 @2x
    ('ic12', 64),   # 32x32 @2x
    ('ic07', 128),  # 128x128
    ('ic13', 256),  # 128x128 @2x
    ('ic08', 256),  # 256x256
    ('ic14', 512),  # 256x256 @2x
    ('ic09', 512),  # 512x512
    ('ic10', 1024), # 512x512 @2x
]

def write_icns(png_by_size, path):
    body = b''
    for tag, size in ICNS_ENTRIES:
        payload = png_by_size[size]
        body += tag.encode('ascii') + struct.pack('>I', len(payload) + 8) + payload
    path.write_bytes(b'icns' + struct.pack('>I', len(body) + 8) + body)


def main():
    OUT.mkdir(exist_ok=True)
    sizes = [16, 32, 48, 64, 128, 256, 512, 1024]
    png_by_size = {size: png_bytes(render(size)) for size in sizes}
    (OUT / 'icon.png').write_bytes(png_by_size[1024])
    write_icns(png_by_size, OUT / 'icon.icns')
    write_ico([(s, png_by_size[s]) for s in (16, 32, 48, 64, 128, 256)], OUT / 'icon.ico')
    for name in ('icon.png', 'icon.icns', 'icon.ico'):
        print(f'wrote {OUT / name} ({(OUT / name).stat().st_size:,} bytes)')


if __name__ == '__main__':
    main()
