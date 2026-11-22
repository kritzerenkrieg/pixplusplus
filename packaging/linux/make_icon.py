"""Draws packaging/linux/pixplusplus-gui.png (N1): a 16x16 pixel-art gem, scaled to 256.

One main colour in three shades plus a warm accent sparkle, the way the balanced palette (M1)
spends its slots. Run: bench/.venv/bin/python packaging/linux/make_icon.py
"""
from pathlib import Path
from PIL import Image

BG = (30, 34, 48, 255)
LIGHT, MID, DARK, OUTLINE = (150, 228, 214), (64, 184, 170), (28, 112, 120), (14, 52, 64)
ACCENT, WHITE = (255, 176, 64), (246, 250, 255)

img = Image.new("RGBA", (16, 16), BG)
for x, y in [(0, 0), (15, 0), (0, 15), (15, 15)]:
    img.putpixel((x, y), (0, 0, 0, 0))  # rounded corners
cx, cy = 7.5, 8.5
for y in range(16):
    for x in range(16):
        d = abs(x - cx) + abs(y - cy)
        if d > 6.5:
            continue
        if d > 5.5:
            c = OUTLINE
        elif x < cx and y < cy:
            c = LIGHT
        elif x > cx and y > cy:
            c = DARK
        else:
            c = MID
        img.putpixel((x, y), c + (255,))
for p in [(5, 6), (6, 5)]:
    img.putpixel(p, WHITE + (255,))  # highlight
for p in [(12, 2), (11, 3), (12, 3), (13, 3), (12, 4)]:
    img.putpixel(p, ACCENT + (255,))  # sparkle
out = Path(__file__).with_name("pixplusplus-gui.png")
img.resize((256, 256), Image.NEAREST).save(out)
print(out)
