"""Joins the screens from readme-shots.js into docs/screens-<scheme>.png:
three phone screens side by side with rounded corners on a clear background.

    python3 firmware/ui/readme-shots.py <dir-with-shots>
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw

DOCS = Path(__file__).resolve().parents[2] / "docs"
FRAME_WIDTH = 492
MARGIN = 30
GAP = 32
RADIUS = 14
BORDER = {"light": (208, 208, 208, 255), "dark": (58, 58, 58, 255)}


def frame(shot: Path, scheme: str) -> Image.Image:
    image = Image.open(shot).convert("RGBA")
    height = round(image.height * FRAME_WIDTH / image.width)
    image = image.resize((FRAME_WIDTH, height), Image.LANCZOS)
    mask = Image.new("L", image.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, FRAME_WIDTH - 1, height - 1), RADIUS, fill=255)
    image.putalpha(mask)
    ImageDraw.Draw(image).rounded_rectangle((0, 0, FRAME_WIDTH - 1, height - 1), RADIUS, outline=BORDER[scheme], width=2)
    return image


def join(shots: Path, scheme: str) -> None:
    frames = [frame(shots / f"{scheme}-{n}.png", scheme) for n in (1, 2, 3)]
    height = max(f.height for f in frames)
    width = 2 * MARGIN + 3 * FRAME_WIDTH + 2 * GAP
    canvas = Image.new("RGBA", (width, height + 2 * MARGIN), (0, 0, 0, 0))
    for n, image in enumerate(frames):
        canvas.alpha_composite(image, (MARGIN + n * (FRAME_WIDTH + GAP), MARGIN))
    canvas.quantize(256, method=Image.Quantize.FASTOCTREE).save(DOCS / f"screens-{scheme}.png", optimize=True)


if __name__ == "__main__":
    for scheme in ("light", "dark"):
        join(Path(sys.argv[1]), scheme)
