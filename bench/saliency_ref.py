"""Reference maps for `src/saliency.rs` (E7).

`spectral_residual` below is copied verbatim from the benchmark harness that produced the
showcase maps (PIXSOTA `bench/run_bench.py`). This script draws two synthetic test images
(no third-party art, so they can be committed) and writes the Python saliency map for each:

  tests/fixtures/saliency_src.png       333 x 217  (horizontal-first resample)
  tests/fixtures/saliency_ref.png
  tests/fixtures/saliency_tall_src.png  120 x 400  (vertical-first resample)
  tests/fixtures/saliency_tall_ref.png

usage: bench/.venv/bin/python bench/saliency_ref.py
"""
from pathlib import Path
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

FIX = Path(__file__).resolve().parent.parent / "tests" / "fixtures"


def spectral_residual(rgb8, out_hw):
    """Hou & Zhang 2007 saliency, used as an automatic importance map."""
    g = Image.fromarray(rgb8).convert("L")
    w, h = g.size
    sw = 64
    sh = max(8, round(h * sw / w))
    x = np.asarray(g.resize((sw, sh), Image.Resampling.BOX), dtype=np.float64) / 255
    F = np.fft.fft2(x)
    logA = np.log(np.abs(F) + 1e-9)
    p = np.pad(logA, 1, mode="wrap")
    avg = sum(p[i:i + sh, j:j + sw] for i in range(3) for j in range(3)) / 9
    sal = np.abs(np.fft.ifft2(np.exp(logA - avg + 1j * np.angle(F)))) ** 2
    im = Image.fromarray(sal / sal.max() * 255).convert("L").resize(out_hw[::-1], Image.Resampling.BILINEAR)
    im = im.filter(ImageFilter.GaussianBlur(radius=out_hw[1] / 40))
    a = np.asarray(im, dtype=np.float64)
    a = (a - a.min()) / (np.ptp(a) + 1e-9)
    return (a * 255).astype(np.uint8)


def synthetic(w, h, seed):
    """Gradient sky, a few flat shapes and some texture: something with salient objects."""
    rng = np.random.default_rng(seed)
    yy, xx = np.mgrid[0:h, 0:w]
    base = np.stack([60 + 120 * yy / h, 90 + 60 * xx / w, 160 - 80 * yy / h], -1)
    base += rng.normal(0, 6, base.shape)
    im = Image.fromarray(np.clip(base, 0, 255).astype(np.uint8))
    d = ImageDraw.Draw(im)
    for _ in range(6):
        x0, y0 = int(rng.integers(0, w - 20)), int(rng.integers(0, h - 20))
        x1, y1 = x0 + int(rng.integers(10, w // 3)), y0 + int(rng.integers(10, h // 3))
        col = tuple(int(c) for c in rng.integers(0, 256, 3))
        (d.ellipse if rng.random() < 0.5 else d.rectangle)([x0, y0, x1, y1], fill=col, outline=(20, 20, 20))
    return np.asarray(im)


for name, (w, h, seed) in {"saliency": (333, 217, 1), "saliency_tall": (120, 400, 2)}.items():
    src = synthetic(w, h, seed)
    Image.fromarray(src).save(FIX / f"{name}_src.png")
    Image.fromarray(spectral_residual(src, src.shape[:2])).save(FIX / f"{name}_ref.png")
    print(f"wrote {name}_src.png / {name}_ref.png ({w}x{h})")
