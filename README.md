# PIARU

Rust implementation of Pixelated Image Abstraction (Gerstner et al., NPAR 2012 / C&G 2013):
superpixels are the output pixels, and segmentation and palette are solved jointly with
mass-constrained deterministic annealing in Oklab.

```sh
cargo build --release
./target/release/piaru art.png -o out.png -s 256 -k 16 --preview 4
#   --importance map.png   grayscale; brighter areas get more palette mass
#   --edge-weight map.png  EXPERIMENTAL weight-in-cost (off unless given)
#   --stats run.json       timings / iterations / peak RSS
#   --pyxeled-compat --space cielab   parity mode against Pyxeled
```

Input is downscaled to ≤ 4× the output (`--input-factor`).

## Parity check
```sh
bench/.venv/bin/python bench/parity.py <source> <pyxeled_out> <piaru_out>...
```
Divergences and parity numbers: `docs/PORTING.md`. The benchmark harness, test images and paper
live in the sibling research project `../PIXSOTA`.

## Development
Developed with [DLAD](https://github.com/kritzerenkrieg/dlad): `DECISIONS.md` is the source of truth,
then `ARCHITECTURE.md` and `ROADMAP.md` (implementation only; the research plan is in
`../PIXSOTA/ROADMAP.md`). Agent entry point: `AGENTS.md`.
