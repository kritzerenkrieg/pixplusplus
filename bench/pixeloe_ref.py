"""Reference dumps of every PixelOE (torch backend) stage, for the Rust port's unit tests (G1-G3).

Runs the upstream `pixeloe.torch` code (the version vendored at refs/PixelOE, kornia for Lab) on
two small synthetic images and writes each intermediate as raw little-endian float32 plus a
manifest with shapes. Needs torch, so run it on the remote host, not the laptop (A5):

  python bench/pixeloe_ref.py OUT_DIR        # then copy OUT_DIR to tests/fixtures/pixeloe/

Images: `a` 48x40 (a multiple of pixel size 4) and `b` 50x38 (exercises the replicate padding).
"""
import json, sys
from pathlib import Path
import numpy as np
import torch
import torch.nn.functional as F
import kornia
import importlib.metadata as md

from pixeloe.torch.lab import rgb_to_lab, lab_to_rgb
from pixeloe.torch.minmax import KERNELS, erode_cont, dilate_cont
from pixeloe.torch.outline import local_stat, expansion_weight, outline_expansion
from pixeloe.torch.sharpen.unsharp import unsharp_mask
from pixeloe.torch.sharpen.laplacian import laplacian_sharpen
from pixeloe.torch.color import (match_color, wavelet_blur, gaussian_kernel, color_quantization_kmeans,
                                 quantize_and_dither)
from pixeloe.torch.downscale.contrast_based import contrast_downscale
from pixeloe.torch.downscale.k_centroid import k_centroid_downscale_torch
from pixeloe.torch.downscale.lanczos import lanczos_resize
from pixeloe.torch.pixelize import pixelize

torch.set_num_threads(1)
OUT = Path(sys.argv[1])
OUT.mkdir(parents=True, exist_ok=True)
manifest = {"versions": {"torch": torch.__version__, "kornia": kornia.__version__, "pixeloe": md.version("pixeloe")},
            "arrays": {}}


def save(name, t):
    a = t.detach().float().contiguous().cpu().numpy() if torch.is_tensor(t) else np.asarray(t, np.float32)
    a.astype("<f4").tofile(OUT / f"{name}.f32")
    manifest["arrays"][name] = list(a.shape)


def synthetic(w, h, seed):
    """Smooth gradients, hard-edged shapes, thin dark lines and mild noise, in [0, 1]."""
    g = torch.Generator().manual_seed(seed)
    yy, xx = torch.meshgrid(torch.linspace(0, 1, h), torch.linspace(0, 1, w), indexing="ij")
    img = torch.stack([0.2 + 0.6 * yy, 0.3 + 0.4 * xx, 0.8 - 0.5 * yy * xx])
    img[:, h // 4: h // 2, w // 5: w // 2] = torch.tensor([0.9, 0.2, 0.15])[:, None, None]
    img[:, h // 2: 3 * h // 4, w // 2: 4 * w // 5] = torch.tensor([0.1, 0.12, 0.3])[:, None, None]
    img[:, h // 3, :] = 0.05
    img[:, :, 2 * w // 3] = 0.95
    img = img + 0.03 * torch.randn(img.shape, generator=g)
    return img.clamp(0, 1)[None]


with torch.inference_mode():
    for tag, (w, h, seed) in {"a": (48, 40, 1), "b": (50, 38, 2)}.items():
        img = synthetic(w, h, seed)
        save(f"{tag}_img", img)
        if tag == "b":
            # Padding path: full pipeline only.
            save("b_pixelize_default", pixelize(img, pixel_size=4, no_post_upscale=True, backend="torch"))
            save("b_pixelize_upscaled", pixelize(img, pixel_size=4, thickness=2, backend="torch"))
            continue

        # --- building blocks
        lab = rgb_to_lab(img)
        save("a_lab", lab)
        save("a_lab_to_rgb", lab_to_rgb(lab))
        wild = lab * torch.tensor([1.1, 1.6, 1.6])[None, :, None, None] + torch.tensor([5.0, 10.0, -10.0])[None, :, None, None]
        save("a_wild_lab", wild)
        save("a_wild_lab_to_rgb", lab_to_rgb(wild))
        for t in range(1, 7):
            save(f"kernel_{t}", KERNELS[t])
        for t in (1, 2, 3):
            k = KERNELS[t]
            save(f"a_erode_{t}", erode_cont(img, k, 1))
            save(f"a_dilate_{t}", dilate_cont(img, k, 1))
            save(f"a_dilate_{t}_x2", dilate_cont(img, k, 2))
        lum = lab[:, 0:1] / 100
        for ks, st in ((8, 2), (4, 2), (5, 3)):
            for stat in ("median", "min", "max"):
                save(f"a_local_{stat}_{ks}_{st}", local_stat(lum, ks, st, stat))

        # --- outline expansion
        for mapping in ("current", "contrast_ratio", "contrast_gated"):
            save(f"a_weight_{mapping}", expansion_weight(img, 4, 2, mapping=mapping))
        save("a_weight_none", expansion_weight(img, 4, 2, normalize="none"))
        for t in (1, 2, 3):
            out, wt = outline_expansion(img, t, t, 4)
            save(f"a_expand_{t}", out)
            save(f"a_expand_{t}_w", wt)
        expanded = outline_expansion(img, 2, 2, 4)[0]

        # --- sharpen
        save("a_unsharp", unsharp_mask(expanded, kernel_size=3, sigma=1.0, amount=0.5))
        save("a_laplacian", laplacian_sharpen(expanded, amount=0.5))

        # --- colour transfer
        for r in (2, 4):
            save(f"gauss_kernel_{r}", gaussian_kernel(r, torch.device("cpu")).float())
            save(f"a_wavelet_blur_{r}", wavelet_blur(expanded, r))
        matched = match_color(expanded, img)
        save("a_match_color", matched)

        # --- downscale modes (on the colour-matched image, as in pixelize)
        save("a_down_contrast", contrast_downscale(matched, 4))
        save("a_down_k_centroid", k_centroid_downscale_torch(matched, 4, 2))
        save("a_down_lanczos", lanczos_resize(matched, size=(10, 12)))
        for mode in ("nearest", "nearest-exact", "bilinear", "bicubic", "area"):
            save(f"a_down_{mode}", F.interpolate(matched, size=(10, 12), mode=mode))

        # --- colour step
        down = contrast_downscale(matched, 4)
        q, pal, _ = color_quantization_kmeans(down, num_centroids=16)
        save("a_kmeans_q", q)
        save("a_kmeans_pal", pal)
        q5, pal5, _ = color_quantization_kmeans(down, num_centroids=5)
        save("a_kmeans5_q", q5)
        for dm in ("ordered", "error_diffusion", "none"):
            save(f"a_dither_{dm}", quantize_and_dither(down, num_centroids=16, dither_method=dm))

        # --- full pipeline configurations (no post upscale unless stated)
        cfgs = {
            "default": {},
            "harness": dict(thickness=2, mode="contrast", do_color_match=True, do_quant=False),
            "t0": dict(thickness=0),
            "nomatch": dict(do_color_match=False),
            "kcentroid": dict(mode="k_centroid"),
            "lanczos": dict(mode="lanczos"),
            "bilinear": dict(mode="bilinear"),
            "unsharp": dict(sharpen_mode="unsharp"),
            "laplacian": dict(sharpen_mode="laplacian"),
            "quant_ordered": dict(do_quant=True, num_colors=16, dither_mode="ordered"),
            "quant_ed": dict(do_quant=True, num_colors=16, dither_mode="error_diffusion"),
            "quant_none": dict(do_quant=True, num_colors=16, dither_mode="none"),
            "quant_weighted": dict(do_quant=True, num_colors=16, quant_mode="weighted-kmeans", dither_mode="none"),
            "quant_repeat": dict(do_quant=True, num_colors=16, quant_mode="repeat-kmeans", dither_mode="none"),
            "map_ratio": dict(weight_mapping="contrast_ratio"),
            "map_gated": dict(weight_mapping="contrast_gated"),
        }
        for name, kw in cfgs.items():
            save(f"a_pixelize_{name}", pixelize(img, pixel_size=4, no_post_upscale=True, backend="torch", **kw))
        manifest["configs"] = cfgs

(OUT / "manifest.json").write_text(json.dumps(manifest, indent=1))
print(json.dumps(manifest["versions"]), len(manifest["arrays"]), "arrays",
      sum((OUT / f"{n}.f32").stat().st_size for n in manifest["arrays"]) // 1024, "KiB")
