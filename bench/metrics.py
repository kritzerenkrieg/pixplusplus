"""Objective metrics for the Pix++ benchmark. All colour maths in Oklab.

Reference for every method = the exact input image the methods received, box-downsampled
to the output grid (what a perfect, palette-unconstrained pixelisation would show).

palette error : per-pixel ΔE_ok between output and reference. Reported ×100
                (Oklab ΔE×100 ≈ CIELAB ΔE in magnitude; ~1 = just noticeable).
edge retention: "saliency" = Gaussian-smoothed gradient magnitude of Oklab L on the
                reference; ridges = non-max-suppressed pixels in its top 20%.
                retention = mean |∇L_out| / mean |∇L_ref| on ridge pixels (1.0 = same contrast,
                >1 = sharper/harder lines, <1 = lines washed out).
                hit_rate  = fraction of ridge pixels where |∇L_out| >= 0.5 |∇L_ref|.
"""

import numpy as np
from PIL import Image


def srgb_to_oklab(rgb8: np.ndarray) -> np.ndarray:
    c = rgb8.astype(np.float64) / 255.0
    c = np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
    M1 = np.array([[0.4122214708, 0.5363325363, 0.0514459929],
                   [0.2119034982, 0.6806995451, 0.1073969566],
                   [0.0883024619, 0.2817188376, 0.6299787005]])
    M2 = np.array([[0.2104542553, 0.7936177850, -0.0040720468],
                   [1.9779984951, -2.4285922050, 0.4505937099],
                   [0.0259040371, 0.7827717662, -0.8086757660]])
    lms = np.cbrt(c @ M1.T)
    return lms @ M2.T


def load_rgb(path) -> np.ndarray:
    return np.asarray(Image.open(path).convert("RGB"))


def reference_at(src_rgb8: np.ndarray, w: int, h: int) -> np.ndarray:
    """Box-filter (area) downsample of the source, in linear light, to (h, w, 3) uint8."""
    im = Image.fromarray(src_rgb8)
    return np.asarray(im.resize((w, h), Image.Resampling.BOX))


def _gauss(img, sigma=0.8):
    r = max(1, int(3 * sigma))
    x = np.arange(-r, r + 1)
    k = np.exp(-x**2 / (2 * sigma**2)); k /= k.sum()
    H, W = img.shape
    p = np.pad(img, ((0, 0), (r, r)), mode="edge")
    tmp = sum(k[i] * p[:, i:i + W] for i in range(2 * r + 1))
    p = np.pad(tmp, ((r, r), (0, 0)), mode="edge")
    return sum(k[i] * p[i:i + H, :] for i in range(2 * r + 1))


def _sobel(L):
    p = np.pad(L, 1, mode="edge")
    gx = (p[:-2, 2:] + 2 * p[1:-1, 2:] + p[2:, 2:]) - (p[:-2, :-2] + 2 * p[1:-1, :-2] + p[2:, :-2])
    gy = (p[2:, :-2] + 2 * p[2:, 1:-1] + p[2:, 2:]) - (p[:-2, :-2] + 2 * p[:-2, 1:-1] + p[:-2, 2:])
    return gx / 8.0, gy / 8.0


def ridge_mask(L_ref, top=0.20):
    S = _gauss(L_ref, 0.8)
    gx, gy = _sobel(S)
    mag = np.hypot(gx, gy)
    ang = (np.rad2deg(np.arctan2(gy, gx)) + 180.0) % 180.0
    p = np.pad(mag, 1, mode="constant")
    h, w = mag.shape
    nb = {0: ((0, 1), (0, -1)), 45: ((-1, 1), (1, -1)), 90: ((1, 0), (-1, 0)), 135: ((1, 1), (-1, -1))}
    q = (np.round(ang / 45.0).astype(int) % 4) * 45
    keep = np.zeros_like(mag, dtype=bool)
    for a, ((dy1, dx1), (dy2, dx2)) in nb.items():
        n1 = p[1 + dy1:1 + dy1 + h, 1 + dx1:1 + dx1 + w]
        n2 = p[1 + dy2:1 + dy2 + h, 1 + dx2:1 + dx2 + w]
        keep |= (q == a) & (mag >= n1) & (mag >= n2)
    thr = np.quantile(mag, 1.0 - top)
    return keep & (mag >= thr) & (mag > 1e-6)


def evaluate(src_rgb8: np.ndarray, out_rgb8: np.ndarray) -> dict:
    h, w = out_rgb8.shape[:2]
    ref = reference_at(src_rgb8, w, h)
    lab_ref = srgb_to_oklab(ref)
    lab_out = srgb_to_oklab(out_rgb8)
    de = np.linalg.norm(lab_ref - lab_out, axis=-1) * 100.0

    mask = ridge_mask(lab_ref[..., 0])
    g_ref = np.hypot(*_sobel(lab_ref[..., 0]))
    g_out = np.hypot(*_sobel(lab_out[..., 0]))
    gr, go = g_ref[mask], g_out[mask]
    colors = np.unique(out_rgb8.reshape(-1, 3), axis=0).shape[0]
    return {
        "de_mean": float(de.mean()),
        "de_p95": float(np.quantile(de, 0.95)),
        "de_max": float(de.max()),
        "edge_retention": float(go.mean() / max(gr.mean(), 1e-9)),
        "edge_hit_rate": float((go >= 0.5 * gr).mean()),
        "ridge_px": int(mask.sum()),
        "colors_used": int(colors),
    }
