#!/usr/bin/env python3
"""Turns the 1024 px masters in design/ into the files the apps ship.  Needs Pillow + numpy, macOS (sips, iconutil).

    python3 design/export.py

  design/art/<id>.png   -> macos/Resources/Art/<id>.heic   transparent margin trimmed, subject ~86% of the frame,
                                                           520 px (largest use is 260 pt @2x), HEIC q80 with alpha
  design/AppIcon.png    -> macos/Resources/AppIcon.icns    squircle on Apple's 824/1024 grid with drop shadow;
                                                           16-64 px use design/AppIcon-small.png if present (a
                                                           bolder, simpler full-bleed 1024 master), else a white
                                                           broom drawn here on a dark tile
                        -> windows/app.ico                 same shapes, tighter margin, all entries PNG

New masters: drop a PNG into design/art/ (or replace design/AppIcon.png) and rerun; `export.py art` or
`export.py icons` does one half. An icon master may be full bleed or already rounded on a transparent
background (it is cropped to its opaque part first). Outputs are committed, so building the apps does not need Pillow.
"""
import glob
import os
import subprocess
import sys
import tempfile

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ART_OUT = f"{ROOT}/macos/Resources/Art"


def resize(im, n):
    """Premultiplied resize, so transparent edges do not pick up dark fringes."""
    return im.convert("RGBa").resize((n, n), Image.LANCZOS).convert("RGBA")


def export_art():
    os.makedirs(ART_OUT, exist_ok=True)
    tmp = tempfile.mkdtemp()
    for f in sorted(glob.glob(f"{ROOT}/design/art/*.png")):
        name = os.path.splitext(os.path.basename(f))[0]
        im = Image.open(f).convert("RGBA")
        l, t, r, b = im.getchannel("A").point(lambda v: 255 if v > 8 else 0).getbbox()
        side = round(max(r - l, b - t) / 0.86)
        x, y = round((l + r - side) / 2), round((t + b - side) / 2)
        png = f"{tmp}/{name}.png"
        resize(im.crop((x, y, x + side, y + side)), 520).save(png)  # crop pads out of bounds with transparency
        subprocess.run(["sips", "-s", "format", "heic", "-s", "formatOptions", "80", png,
                        "--out", f"{ART_OUT}/{name}.heic"], check=True, stdout=subprocess.DEVNULL)


def squircle(n, inset):
    """Antialiased superellipse (n=5, close to Apple's continuous corners) mask, n px, inset px each side."""
    ss = 4
    c = (np.arange(n * ss) + 0.5) / ss
    half = n / 2 - inset
    u = np.abs(c - n / 2) / half
    inside = (u[None, :] ** 5 + u[:, None] ** 5) <= 1
    return Image.fromarray((inside * 255).astype(np.uint8)).resize((n, n), Image.BOX)


def gradient(n, a, b):
    """Diagonal gradient from colour a (top left) to b (bottom right)."""
    t = (np.arange(n)[None, :] + np.arange(n)[:, None]) / (2 * n - 2)
    rgb = np.array(a)[None, None, :] * (1 - t[..., None]) + np.array(b)[None, None, :] * t[..., None]
    return Image.fromarray(rgb.astype(np.uint8), "RGB").convert("RGBA")


def broom_mask(n, box):
    """Bold tilted broom silhouette for small sizes: fitted into a box px square centred on an n px canvas."""
    k = 4  # drawn at 4x, in 1024 units
    m = Image.new("L", (1024 * k, 1024 * k), 0)
    d = ImageDraw.Draw(m)
    P = lambda *pts: [(x * k, y * k) for x, y in pts]
    hw = 60 if n < 32 else 40  # handle half width: thicker when tiny or it vanishes
    d.rounded_rectangle(P((512 - hw, 130), (512 + hw, 470)), radius=hw * k, fill=255)  # handle
    d.rounded_rectangle(P((382, 440), (642, 560)), radius=56 * k, fill=255)  # ferrule
    d.polygon(P((372, 582), (652, 582), (740, 840), (512, 900), (284, 840)), fill=255)  # bristles
    if n >= 32:
        d.line(P((470, 610), (430, 860)), fill=0, width=16 * k)
        d.line(P((554, 610), (594, 860)), fill=0, width=16 * k)
    m = m.rotate(-35, resample=Image.BICUBIC)
    m = m.crop(m.getbbox())
    f = box / max(m.size)
    m = m.resize((max(1, round(m.width * f)), max(1, round(m.height * f))), Image.LANCZOS)
    out = Image.new("L", (n, n), 0)
    out.paste(m, ((n - m.width) // 2, (n - m.height) // 2))
    return out


def icon(n, master, small, inset_frac, shadow):
    """One icon size. inset_frac: transparent margin per side as a fraction of n."""
    inset = n * inset_frac
    tile = round(n - 2 * inset)
    if n <= 64 and small:
        face = Image.new("RGBA", (n, n), (0, 0, 0, 0))
        face.paste(resize(small, tile), (round(inset), round(inset)))
    elif n <= 64:  # detail is unreadable this small
        face = gradient(n, (22, 112, 214), (62, 40, 160))
        face.paste((255, 255, 255, 255), (0, 0), broom_mask(n, tile * 0.78))
    else:
        face = Image.new("RGBA", (n, n), (0, 0, 0, 0))
        face.paste(resize(master, tile), (round(inset), round(inset)))
    shape = squircle(n, inset)
    out = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    if shadow and n >= 32:
        sh = Image.new("RGBA", (n, n), (0, 0, 0, 0))
        sh.putalpha(shape.point(lambda v: v * 0.3))
        sh = sh.transform((n, n), Image.AFFINE, (1, 0, 0, 0, 1, -n * 10 / 1024)).filter(ImageFilter.GaussianBlur(n * 12 / 1024))
        out.alpha_composite(sh)
    face.putalpha(shape)
    out.alpha_composite(face)
    return out


def export_icons():
    def load(path):
        im = Image.open(path).convert("RGBA")
        return im.crop(im.getchannel("A").point(lambda v: 255 if v > 8 else 0).getbbox())
    master = load(f"{ROOT}/design/AppIcon.png")
    small = load(f"{ROOT}/design/AppIcon-small.png") if os.path.exists(f"{ROOT}/design/AppIcon-small.png") else None
    with tempfile.TemporaryDirectory() as tmp:
        iconset = f"{tmp}/AppIcon.iconset"
        os.mkdir(iconset)
        for pt in (16, 32, 128, 256, 512):
            for scale, suffix in ((1, ""), (2, "@2x")):
                icon(pt * scale, master, small, 100 / 1024, True).save(f"{iconset}/icon_{pt}x{pt}{suffix}.png")
        subprocess.run(["iconutil", "-c", "icns", iconset, "-o", f"{ROOT}/macos/Resources/AppIcon.icns"], check=True)
    sizes = (16, 20, 24, 32, 40, 48, 64, 96, 256)
    frames = [icon(n, master, small, 1 / 32, n >= 96) for n in sizes]
    frames[-1].save(f"{ROOT}/windows/app.ico", sizes=[(n, n) for n in sizes], append_images=frames[:-1])


if __name__ == "__main__":
    what = sys.argv[1:] or ["art", "icons"]
    if "art" in what:
        export_art()
    if "icons" in what:
        export_icons()
