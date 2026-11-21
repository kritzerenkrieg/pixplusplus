# Porting notes: Pyxeled → Pix++

Objective, constants and loop structure are ported from `gskaggs/Pyxeled/pyxeled.py`.

| Constant | Value | Note |
|---|---|---|
| SLIC compactness m | 45 | spatial weight = m·√(N/M) |
| T0 / T_final / α | 35 / 1 / 0.7 | |
| ε_palette / ε_cluster | 1 / 0.25 | ε_palette is the **sum** of palette moves |
| perturbation | 1.5 × first principal axis | |
| Laplacian | 0.4 | 4-neighbour |
| bilateral | 0.5 blend, w = exp(−\|ΔL\|), 8-neighbour | |
| saturation | a,b × 1.1 | benchmark runs 1.0 |

Working space is Oklab × 100, so these CIELAB-tuned constants stay on the same scale.

## Pyxeled bugs, fixed by default (all kept under `--pyxeled-compat`)
1. `Color.perturb` adds `delta[1]` to the b channel (should be `delta[2]`).
2. The superpixel grid is anchored at cell top-left corners, not centres.
3. `normalize_probs` writes the cluster average to the misspelled `palette_color`, so it is
   never used. The effective behaviour (argmax sub-colour) is what Pix++ does in both modes.
   The "intended" code would have used *global* cluster mass, which gives every superpixel
   the same colour, so it is not ported.
4. If annealing ends before K reaches K_max, Pyxeled outputs up to 2·K sub-colours, which can
   break the palette limit. Pix++ merges any remaining pairs and runs one last association,
   so the limit holds.
5. Output colours are the palette *before* the final refinement (kept in compat only).

## Parity check (milestone 2)
`bench/parity.py` on Pyxeled's own example (mountains, 614×396 → 128×83, K = 64, CIELAB):

| run | colours | ΔE→source | ΔE vs Pyxeled out | px within ΔE 5 | palette dist |
|---|---|---|---|---|---|
| Pyxeled reference | 42 | 3.13 | — | — | — |
| Pix++ compat, f64 | 41 | 3.10 | 0.46 | 97 % | 0.34 / 0.40 |
| Pix++ compat, f32 | 29 | 3.17 | 1.60 | 94 % | 0.87 / 1.33 |
| Pix++ Oklab, no importance map (the default until Q4) | 18 | 2.64 | 3.68 | 82 % | — |

(ΔE values are Oklab × 100.) The f32 gap comes from split decisions near ε_cluster: precision
changes which clusters split, but not output quality.

The f64 row is measured with a temporary `pub type F = f64` in `src/color.rs`, a release rebuild,
and a revert; there is no runtime flag for it. The shipped build is f32 (B2), so the routine check
is the f32 row. Re-measured 2026-11-15: f32 1.60 and f64 0.46.

## Observed behaviour of the ported method (not changed yet)
* Splits are driven by the perturbation, not by phase transitions. A perturbation of 1.5 is
  already above ε_cluster = 0.25, and convergence is a *sum* threshold, so K doubles at every
  early temperature step.
* The palette is under-used: at K = 16, typically only 9–13 entries win any superpixel.

## PixelOE port (Phase G)
`src/pixeloe/` ports the torch backend of PixelOE 1.0.0 (`refs/PixelOE`, commit 7ce444b; Lab
via kornia 0.9.2). Each stage is checked against dumps from `bench/pixeloe_ref.py`
(`tests/fixtures/pixeloe/`), within 1e-5 on 0..1 data (G5). Divergences from upstream:

1. **k-centroid ties (G6).** When a patch's 2-means splits evenly, both centroids have the same
   total squared distance and torch's pick depends on its float summation order. Pix++ gives a
   tie (relative gap < 1e-5) to the first centroid, the one started from the patch maximum.
   On the 48×40 test image, 16 of 120 patches tie; the other 104 match exactly.
2. **No colour match after quantising (G10).** Upstream follows k-means with
   `match_color(quantised, down)`, whose wavelet colour fix brings back continuous tones (568
   colours at `-k 16` on the smoke image). The CLI skips it, so `-k N` is a hard limit as in PIA;
   `--oe-post-match` restores upstream. The library default (`Options::default()`) is upstream.
3. **Edge cases where upstream raises an error (2026-11-20).** Pix++ runs them instead; every
   other input is unchanged. Pixel size 1: the `local_stat` stride `k // 2` is 0, so Pix++ uses 1.
   `-k 1` with ordered dither: no second colour to dither towards, so each pixel takes the one
   colour. Error diffusion on a 1-pixel-wide or 1-pixel-tall image: reflection on a length-1 axis
   stays at index 0. `--oe-thickness` is limited to 0–6, the sizes upstream has kernels for.
   Weighted or repeat k-means after one-axis padding (item 4) with the contrast or k-centroid
   downscale: upstream sizes the weights to the padded output grid, which has more cells than
   the downscaled image, and torch fails; Pix++ sizes them to the downscaled image.
4. **Kept on purpose: padding grows both axes.** When only one axis needs padding to a multiple
   of the pixel size, upstream still adds a cell to both (`pixelize.py`, `out_h += 1; out_w += 1`).
   Pix++ keeps this for parity (Q3). The CLI and GUI resize PixelOE input to an exact multiple
   (G7), so it only happens with `--input-factor 0`.
5. **K-means sums in f64 (2026-11-21).** torch's `sum` is pairwise, so it stays accurate over
   many pixels; a naive f32 running sum drifts (a constant 204/255 image at 512×512 gave a
   203 centroid). Pix++ accumulates the cluster sums in f64, which is closer to upstream.

### PixelOE parity (G3, G5), 2026-11-18
`bench/pixeloe_parity.py` on the 6 showcase images, default options, pixel size 4, same
Pillow-resized input for both. Run on the main GPU server (CPU only, 12 cores; torch 2.15.0+cpu
with 6 threads, kornia 0.9.0, pixeloe 1.0.0). ΔE is Oklab ×100 between the two 8-bit outputs.

| image | size | mean ΔE | max ΔE | identical | torch ms | pixplusplus ms | torch peak MiB | pixplusplus peak MiB |
|---|---|---|---|---|---|---|---|---|
| anime_hood | 128 | 0.000 | 0.1 | 99.98 % | 317 | 868 | 503 | 27 |
| anime_hood | 512 | 0.000 | 1.8 | 99.99 % | 6006 | 14946 | 1598 | 354 |
| anime_red | 128 | 0.000 | 0.2 | 99.99 % | 333 | 798 | 500 | 26 |
| anime_red | 512 | 0.000 | 1.7 | 99.99 % | 5396 | 13523 | 1531 | 335 |
| land_castle | 128 | 0.000 | 0.0 | 100.00 % | 321 | 858 | 501 | 27 |
| land_castle | 512 | 0.000 | 0.4 | 100.00 % | 5863 | 14883 | 1558 | 346 |
| land_wreck | 128 | 0.000 | 0.0 | 100.00 % | 330 | 874 | 505 | 28 |
| land_wreck | 512 | 0.000 | 0.6 | 99.99 % | 6123 | 15584 | 1596 | 354 |
| toon_chibi | 128 | 0.001 | 6.4 | 99.99 % | 318 | 875 | 502 | 27 |
| toon_chibi | 512 | 0.000 | 1.7 | 100.00 % | 6008 | 14925 | 1583 | 350 |
| toon_penguin | 128 | 0.000 | 0.1 | 99.98 % | 385 | 946 | 510 | 29 |
| toon_penguin | 512 | 0.000 | 0.6 | 99.98 % | 6789 | 16270 | 1667 | 375 |

12/12 within G5; worst mean ΔE 0.001. The few differing pixels are float-order effects (median
and argmin flips). Pix++ uses about 4.5× less peak memory but is about 2.5× slower: the port
is a straight transcription, mostly single-threaded (only the wavelet blur uses rayon).

### PixelOE speed (G11), 2026-11-18
Vectorised wavelet blur (same per-pixel summation order) and row-parallel morphology. All 12
parity outputs are byte-identical to the run above. Same server:

| size | Pix++ before | Pix++ now | torch | Pix++ peak MiB |
|---|---|---|---|---|
| 128 | 798–946 ms | 249–286 ms | 317–385 ms | 28–32 |
| 512 | 13.5–16.3 s | 3.8–4.7 s | 5.4–6.8 s | 369–422 |
