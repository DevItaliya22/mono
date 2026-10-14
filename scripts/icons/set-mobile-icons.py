#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Derive the native mobile app icons from the canonical Koru logo.

The source of truth remains assets/logo.png, which is also embedded by the
desktop application. Run this script after changing that file; do not edit the
generated Android or iOS PNGs independently.
"""

import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    sys.exit(
        "Pillow is required to rebuild the mobile icons.\n"
        "    py -3 -m pip install Pillow"
    )


ROOT = Path(__file__).resolve().parent.parent.parent
LOGO = ROOT / "assets" / "logo.png"
ANDROID_RES = ROOT / "apps" / "android" / "app" / "src" / "main" / "res"
IOS_ICON = (
    ROOT
    / "apps"
    / "ios"
    / "KoruMobile"
    / "Assets.xcassets"
    / "AppIcon.appiconset"
    / "AppIcon.png"
)

# A quiet neutral tint lets the blue/green mark retain its soft transparent
# edges while giving launchers the opaque surface required for an app icon.
BACKGROUND = (244, 250, 249, 255)
ANDROID_LEGACY_SIZES = {
    "mipmap-mdpi": 48,
    "mipmap-hdpi": 72,
    "mipmap-xhdpi": 96,
    "mipmap-xxhdpi": 144,
    "mipmap-xxxhdpi": 192,
}


def on_background(logo: Image.Image, size: int, fraction: float) -> Image.Image:
    canvas = Image.new("RGBA", (size, size), BACKGROUND)
    mark_size = round(size * fraction)
    mark = logo.resize((mark_size, mark_size), Image.Resampling.LANCZOS)
    offset = ((size - mark_size) // 2, (size - mark_size) // 2)
    canvas.alpha_composite(mark, offset)
    return canvas.convert("RGB")


def transparent_foreground(logo: Image.Image, size: int, fraction: float) -> Image.Image:
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    mark_size = round(size * fraction)
    mark = logo.resize((mark_size, mark_size), Image.Resampling.LANCZOS)
    offset = ((size - mark_size) // 2, (size - mark_size) // 2)
    canvas.alpha_composite(mark, offset)
    return canvas


def save(image: Image.Image, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    image.save(path, format="PNG", optimize=True)
    print(f"icon    {path.relative_to(ROOT)}  {image.width}x{image.height}")


def main() -> int:
    if not LOGO.is_file():
        sys.exit(f"no canonical logo at {LOGO}")

    logo = Image.open(LOGO).convert("RGBA")
    if logo.width != logo.height:
        sys.exit(f"mobile logo must be square, got {logo.width}x{logo.height}")

    for directory, size in ANDROID_LEGACY_SIZES.items():
        save(on_background(logo, size, 0.82), ANDROID_RES / directory / "ic_launcher.png")
        save(on_background(logo, size, 0.82), ANDROID_RES / directory / "ic_launcher_round.png")

    # An adaptive icon layer is 108dp (432 px at xxxhdpi). Keep the mark in
    # Android's 66dp safe zone so masks and launcher motion never clip it.
    save(
        transparent_foreground(logo, 432, 0.61),
        ANDROID_RES / "drawable-xxxhdpi" / "ic_launcher_foreground.png",
    )

    # App Store icons cannot contain alpha. The same neutral background is
    # flattened here, with enough inset to survive every system mask.
    save(on_background(logo, 1024, 0.82), IOS_ICON)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
