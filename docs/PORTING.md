# Porting notes: Pyxeled → PIARU

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
   never used. The effective behaviour (argmax sub-colour) is what PIARU does in both modes.
   The "intended" code would have used *global* cluster mass, which gives every superpixel
   the same colour, so it is not ported.
4. If annealing ends before K reaches K_max, Pyxeled outputs up to 2·K sub-colours, which can
   break the palette limit. PIARU merges any remaining pairs and runs one last association,
   so the limit holds.
5. Output colours are the palette *before* the final refinement (kept in compat only).

## Parity check (milestone 2)
`bench/parity.py` on Pyxeled's own example (mountains, 614×396 → 128×83, K = 64, CIELAB):

| run | colours | ΔE→source | ΔE vs Pyxeled out | px within ΔE 5 | palette dist |
|---|---|---|---|---|---|
| Pyxeled reference | 42 | 3.13 | — | — | — |
| PIARU compat, f64 | 41 | 3.10 | 0.46 | 97 % | 0.34 / 0.40 |
| PIARU compat, f32 | 29 | 3.17 | 1.60 | 94 % | 0.87 / 1.33 |
| PIARU Oklab (default) | 18 | 2.64 | 3.68 | 82 % | — |

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
