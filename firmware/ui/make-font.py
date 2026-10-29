#!/usr/bin/env python3
"""Builds ui/operator-mono.woff: Cascadia Code cut down to what the page needs.

Keeps weights 400-700 (one variable font), Latin, Ukrainian (and the rest of
basic Cyrillic), digits and typographic punctuation. Fails if index.html uses
a character the subset lacks.

Cascadia Code's OFL licence reserves the name "Cascadia Code", so the
modified font is renamed "Operator Mono" (see OFL.txt).

Usage: python3 make-font.py [/usr/share/fonts/TTF/CascadiaCode.ttf]
"""
import pathlib
import sys

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

HERE = pathlib.Path(__file__).parent
SOURCE = sys.argv[1] if len(sys.argv) > 1 else "/usr/share/fonts/TTF/CascadiaCode.ttf"
NAME = "Operator Mono"

CHARS = (
    list(range(0x20, 0x7F))          # ASCII
    + list(range(0x400, 0x460))      # Cyrillic: А–я, Є, І, Ї, …
    + [0x490, 0x491]                 # Ґ ґ
    + [0xA0, 0xAB, 0xBB, 0xB0, 0xB7, 0xD7, 0x2BC]  # nbsp « » ° · × ʼ
    + [0x2013, 0x2014, 0x2018, 0x2019, 0x201C, 0x201D, 0x201E, 0x2026]  # – — ‘ ’ “ ” „ …
    + [0x2116, 0x2190, 0x2191, 0x2192, 0x2193, 0x2713]  # № ← ↑ → ↓ ✓
)


# Emoji are drawn by the system's emoji font (design_system/typography.md
# allows it as the fallback), so they are not part of the subset:
# pictographs, zero-width joiner, variation selector, gender signs.
EMOJI = set(range(0x1F000, 0x1FB00)) | {0x200D, 0xFE0F, 0x2640, 0x2642}


def page_chars():
    """Characters of index.html outside the page's own markup-only ASCII."""
    text = (HERE / "index.html").read_text(encoding="utf-8")
    return {ord(c) for c in text if ord(c) > 0x7E} - EMOJI


def main():
    missing = sorted(page_chars() - set(CHARS))
    if missing:
        sys.exit("index.html uses characters outside the font: " + " ".join(f"{chr(c)} U+{c:04X}" for c in missing))

    font = TTFont(SOURCE)
    options = subset.Options()
    options.layout_features = []      # no coding ligatures: plain monospace text
    options.name_IDs = ["*"]
    options.flavor = "woff"
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=CHARS)
    subsetter.subset(font)
    font = instancer.instantiateVariableFont(font, {"wght": (400, 700)})
    rename(font)
    out = HERE / "operator-mono.woff"
    font.flavor = "woff"
    font.save(out)
    print(f"{out.name}: {out.stat().st_size} bytes, {len(font.getGlyphOrder())} glyphs")


def rename(font):
    for record in font["name"].names:
        text = record.toUnicode()
        text = text.replace("Cascadia Code", NAME).replace("CascadiaCode", NAME.replace(" ", ""))
        record.string = text


if __name__ == "__main__":
    main()
