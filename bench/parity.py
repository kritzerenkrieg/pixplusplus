"""Milestone 2 correctness check: Pix++ vs the Pyxeled reference output on Pyxeled's own example.

usage: parity.py SOURCE PYXELED_OUT PIXPLUSPLUS_OUT [PIXPLUSPLUS_OUT ...]
"""
import sys
import numpy as np
from metrics import load_rgb, srgb_to_oklab, evaluate


def palette_distance(a, b):
    """Mean distance from each colour of palette a to its nearest colour in b (ΔE_ok×100)."""
    la, lb = srgb_to_oklab(a[None])[0], srgb_to_oklab(b[None])[0]
    d = np.linalg.norm(la[:, None] - lb[None], axis=-1) * 100
    return float(d.min(1).mean())


src = load_rgb(sys.argv[1])
ref = load_rgb(sys.argv[2])
pal_ref = np.unique(ref.reshape(-1, 3), axis=0)
print(f"{'image':28s} {'colors':>6s} {'ΔE→src':>7s} {'ΔEmax':>6s} {'edge':>5s} | {'ΔE vs Pyxeled':>13s} {'<5':>5s} {'pal→ref':>7s} {'ref→pal':>7s}")
for path in [sys.argv[2]] + sys.argv[3:]:
    out = load_rgb(path)
    m = evaluate(src, out)
    de = np.linalg.norm(srgb_to_oklab(out) - srgb_to_oklab(ref), axis=-1) * 100
    pal = np.unique(out.reshape(-1, 3), axis=0)
    print(f"{path.split('/')[-1]:28s} {m['colors_used']:6d} {m['de_mean']:7.2f} {m['de_max']:6.1f} {m['edge_retention']:5.2f} | "
          f"{de.mean():13.2f} {(de < 5).mean()*100:4.0f}% {palette_distance(pal, pal_ref):7.2f} {palette_distance(pal_ref, pal):7.2f}")
