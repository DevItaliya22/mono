#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Regenerate assets/document.ico from the Phosphor glyph.

The taskbar's **Recent** list shows one row per document, and a shell link's
icon is a *file path plus an index* -- Windows cannot be handed a font glyph. So
the glyph is rasterised once, here, into a multi-size .ico that
`apps/koru/build.rs` embeds as resource 2 and `taskbar.rs` points each Recent
entry at.

Run it when the icon or its colour changes; the output is committed, because a
build machine has no Pillow and `assets/fonts/*.ttf` is fetched rather than
tracked:

    py -3 scripts/icons/make-document-icon.py
    cargo build -p koru

The codepoint is lifted from `crates/koru-draw/src/icons.rs` rather than typed
in twice -- those values come from the pinned Phosphor stylesheet and are never
guessed.
"""
import re
import sys
from pathlib import Path

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:
    sys.exit(
        "Pillow is required to rebuild the icon.\n"
        "  py -3 -m pip install --user pillow"
    )

ROOT = Path(__file__).resolve().parent.parent.parent
FONT = ROOT / "assets" / "fonts" / "Phosphor.ttf"
ICONS_RS = ROOT / "crates" / "koru-draw" / "src" / "icons.rs"
OUT = ROOT / "assets" / "document.ico"

# Every size Explorer and the taskbar ask for. The largest is rendered and the
# rest are resampled from it, which keeps the shape identical at every scale.
SIZES = [256, 128, 64, 48, 32, 24, 16]

# The name in icons.rs, and the ink. A jump list row is drawn on the shell's own
# dark flyout, so a mid grey reads on both the light and dark taskbar without
# either disappearing.
GLYPH = "DOCUMENT"
INK = (0xC8, 0xC8, 0xCC, 0xFF)


def codepoint(name: str) -> str:
    """The glyph, read out of icons.rs so the two cannot drift."""
    source = ICONS_RS.read_text(encoding="utf-8")
    match = re.search(
        rf"pub const {name}: char = '\\u\{{([0-9A-Fa-f]+)\}}'", source
    )
    if not match:
        sys.exit(f"{name} is not in {ICONS_RS.relative_to(ROOT)}")
    return chr(int(match.group(1), 16))


def main() -> None:
    if not FONT.exists():
        sys.exit(
            f"{FONT.relative_to(ROOT)} is missing -- assets/fonts/*.ttf is fetched,\n"
            "not tracked. Run: koru.bat fonts"
        )
    glyph = codepoint(GLYPH)

    # Rendered oversized and trimmed, because a Phosphor glyph does not fill its
    # em box -- centring the *box* would leave the mark small and off-centre.
    canvas = SIZES[0]
    font = ImageFont.truetype(str(FONT), canvas)
    scratch = Image.new("RGBA", (canvas * 2, canvas * 2), (0, 0, 0, 0))
    ImageDraw.Draw(scratch).text((canvas // 2, canvas // 2), glyph, font=font, fill=INK)
    box = scratch.getbbox()
    if box is None:
        sys.exit(f"{GLYPH} rendered as nothing -- is Phosphor.ttf complete?")
    mark = scratch.crop(box)

    # A little air, so the icon is not flush to the edge of its cell.
    pad = round(canvas * 0.08)
    side = max(mark.size) + pad * 2
    square = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    square.paste(
        mark,
        ((side - mark.width) // 2, (side - mark.height) // 2),
        mark,
    )
    square = square.resize((canvas, canvas), Image.LANCZOS)

    OUT.parent.mkdir(parents=True, exist_ok=True)
    square.save(OUT, format="ICO", sizes=[(s, s) for s in SIZES])
    print(f"wrote {OUT.relative_to(ROOT)} ({', '.join(str(s) for s in SIZES)})")


if __name__ == "__main__":
    main()

