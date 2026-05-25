"""Normalize the source PNG so the icon fills its tile edge-to-edge.

Windows scales the icon down to 32x32 (or 16x16) for the taskbar; if
the source has lots of empty padding around the artwork, the visible
glyph ends up tiny and pixelated next to apps that fill their tiles.

This script:
  1. Crops the fully-transparent border off the input.
  2. Re-pads with a controlled, uniform margin (defaults to 6% of the
     longest side -- enough to avoid clipping at small sizes without
     wasting tile real estate).
  3. Centers the result in a square canvas at 1024x1024.
  4. Writes back over the input PNG, ready for `tauri icon`.

Run from the workspace root:
    python src-tauri/icons/generate.py qobee.png
"""

from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image


CANVAS_SIZE = 1024
MARGIN_RATIO = 0.06  # 6% margin on the largest side; tweak if needed.


def normalize(src_path: Path, dst_path: Path) -> None:
    img = Image.open(src_path).convert("RGBA")
    bbox = img.getbbox()  # tightest bounding box of non-transparent pixels
    if bbox is None:
        raise SystemExit(f"{src_path}: image is fully transparent")
    cropped = img.crop(bbox)

    # Compute the target glyph size that leaves a 2*margin gap inside
    # the canvas and preserves aspect ratio.
    margin = int(CANVAS_SIZE * MARGIN_RATIO)
    inner = CANVAS_SIZE - 2 * margin
    cw, ch = cropped.size
    scale = min(inner / cw, inner / ch)
    new_w = max(1, int(round(cw * scale)))
    new_h = max(1, int(round(ch * scale)))
    resized = cropped.resize((new_w, new_h), Image.LANCZOS)

    canvas = Image.new("RGBA", (CANVAS_SIZE, CANVAS_SIZE), (0, 0, 0, 0))
    ox = (CANVAS_SIZE - new_w) // 2
    oy = (CANVAS_SIZE - new_h) // 2
    canvas.paste(resized, (ox, oy), resized)

    canvas.save(dst_path, format="PNG", optimize=True)
    print(
        f"normalized {src_path} -> {dst_path} "
        f"(crop {bbox}, glyph {new_w}x{new_h}, canvas {CANVAS_SIZE}x{CANVAS_SIZE})"
    )


def main() -> None:
    if len(sys.argv) < 2:
        print("usage: generate.py <input.png> [output.png]")
        raise SystemExit(2)
    src = Path(sys.argv[1])
    dst = Path(sys.argv[2]) if len(sys.argv) >= 3 else src
    normalize(src, dst)


if __name__ == "__main__":
    main()
