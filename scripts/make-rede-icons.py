#!/usr/bin/env python3
"""Builds Rede's icons from its transparent logo (packaging/rede/source/rede-logo.png).

The logo sits on a creme tile, used as is. Like Ond's: on macOS the tile is
exactly Apple's rounded square, 824 x 824 centred on a 1024 canvas with no
shadow (macOS 26 and later shrink any other shape onto a grey tile); on Linux
and Windows it is a circle filling the canvas, with a thin rim so it holds its
edge on a light panel.

    python3 scripts/make-rede-icons.py

Writes packaging/rede/macos/{icon-1024,icon-256}.png, packaging/rede/linux/
hicolor/*/apps/dev.solidsilver.rede.png and packaging/rede/windows/rede.ico.
Then run `scripts/make-icns.sh packaging/rede/macos/icon-1024.png Rede
packaging/rede/macos` for the bundle icon. Needs Pillow.
"""

import math
import pathlib

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "packaging/rede"
CREME = (244, 232, 212, 255)
RIM = (226, 210, 186, 255)
SCALE = 4  # draw large, then reduce, for smooth edges
LANCZOS = Image.Resampling.LANCZOS


def logo():
    art = Image.open(OUT / "source/rede-logo.png").convert("RGBA")
    # Sized by the solid artwork: the near-clear pixels at the canvas edges
    # would otherwise count as part of it and shrink the logo on its tile.
    solid = art.getchannel("A").point(lambda v: 255 if v > 128 else 0)
    return art.crop(solid.getbbox())


def squircle(n, side):
    """Apple's icon tile, close to a superellipse with exponent 5."""
    mask = Image.new("L", (n, n), 0)
    pts = []
    for i in range(4000):
        t = 2 * math.pi * i / 4000
        c, s = math.cos(t), math.sin(t)
        x = math.copysign(abs(c) ** 0.4, c)
        y = math.copysign(abs(s) ** 0.4, s)
        pts.append((n / 2 + x * side / 2, n / 2 + y * side / 2))
    ImageDraw.Draw(mask).polygon(pts, fill=255)
    return mask


def place(canvas, art, height):
    n = canvas.width
    width = round(art.width * height / art.height)
    glyph = art.resize((width, height), LANCZOS)
    canvas.alpha_composite(glyph, ((n - width) // 2, (n - height) // 2))


def macos(art):
    n = 1024 * SCALE
    side = 824 * SCALE
    canvas = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    canvas.paste(Image.new("RGBA", (n, n), CREME), (0, 0), squircle(n, side))
    place(canvas, art, round(side * 0.70))
    return canvas.resize((1024, 1024), LANCZOS)


def round_icon(art):
    n = 1024 * SCALE
    canvas = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(canvas)
    edge = round(n * 0.01)
    d.ellipse((edge, edge, n - edge, n - edge), fill=RIM)
    rim = round(n * 0.012)
    d.ellipse((edge + rim, edge + rim, n - edge - rim, n - edge - rim), fill=CREME)
    place(canvas, art, round(n * 0.60))
    return canvas.resize((1024, 1024), LANCZOS)


def main():
    art = logo()
    mac = macos(art)
    (OUT / "macos").mkdir(parents=True, exist_ok=True)
    mac.save(OUT / "macos/icon-1024.png")
    mac.resize((256, 256), LANCZOS).save(OUT / "macos/icon-256.png")

    disc = round_icon(art)
    for size in (16, 22, 24, 32, 48, 64, 96, 128, 256, 512):
        d = OUT / f"linux/hicolor/{size}x{size}/apps"
        d.mkdir(parents=True, exist_ok=True)
        disc.resize((size, size), LANCZOS).save(d / "dev.solidsilver.rede.png")
    (OUT / "windows").mkdir(parents=True, exist_ok=True)
    disc.save(OUT / "windows/rede.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
    print("wrote", OUT)


if __name__ == "__main__":
    main()
