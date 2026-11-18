# PIARU

Rust implementation of Pixelated Image Abstraction (Gerstner et al., NPAR 2012 / C&G 2013):
superpixels are the output pixels, and segmentation and palette are solved jointly with
mass-constrained deterministic annealing in Oklab.

```sh
cargo build --release
./target/release/piaru art.png -o out.png -s 256 -k 16 --preview 4
#   --importance map.png   grayscale; brighter areas get more palette mass
#   --importance auto      spectral-residual saliency computed from the input
#   --stats run.json       timings / iterations / peak RSS
#   --pyxeled-compat --space cielab   parity mode against Pyxeled
```

PixelOE (Apache-2.0, ported from its torch backend; see `LICENSES/`) is the second method:

```sh
./target/release/piaru art.png -o out.png -s 128 --method pixeloe          # unquantised
./target/release/piaru art.png -o out.png -s 128 --method pixeloe -k 16    # k-means + dither
#   --oe-thickness 3  --oe-mode contrast|k-centroid|lanczos|nearest|bilinear|...
#   --oe-sharpen unsharp|laplacian  --oe-dither ordered|error-diffusion|none  --oe-no-match
#   --oe-post-match   upstream's colour fix after -k (output then exceeds k colours, G10)
```

Input is downscaled to ≤ 4× the output (`--input-factor`); under PixelOE it is resized to exactly that, and the factor is the pixel size (G7).

## Parity check
```sh
bench/.venv/bin/python bench/parity.py <source> <pyxeled_out> <piaru_out>...
```
Divergences and parity numbers: `docs/PORTING.md`. PixelOE parity: `bench/pixeloe_parity.py`.

## Development
Developed with [DLAD](https://github.com/kritzerenkrieg/dlad): `DECISIONS.md` is the source of truth,
then `ARCHITECTURE.md` and `ROADMAP.md`. Agent entry point: `AGENTS.md`.
