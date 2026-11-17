# Decisions

Source of truth: overrules `ARCHITECTURE.md` and `ROADMAP.md`. A code is the roadmap phase letter plus a sequence number. Codes are stable and never reused, and a new feature takes the next free letter. A phase starts only when its rows are Decided.

**Status:** **Decided**: binding, with the reason in a few words. **Inspect**: research or a spike first. **TBD**: ask, don't guess. **Question**: not yet read by the human, so treat it as TBD.

**Phases:** A Foundations (done), B Core solver and parity (done), C Controls and harness (done), D Benchmark run and report (done), E Post-benchmark features, G PixelOE in Rust. Letter F is skipped so PIARU codes never collide with PIXSOTA's `F` rows.

**Scope:** this file covers the implementation only. The research project (paper, literature review, benchmark harness, human study) lives in the sibling `../PIXSOTA` checkout with its own `DECISIONS.md` (F rows). The research rows `C3`, `C4` and `D1`–`D4` moved there as `F26`–`F31` (2026-11-17); their stubs below keep the codes from being reused.

## Current state (2026-11-17)

- **Methods (E8):** base PIA (default), PIA + importance map (`--importance FILE|auto`), PixelOE (`--method pixeloe`, to be ported).
- **Next work:** cut the edge weight (E1), format once (E4), `--importance auto` (E7), then the PixelOE port (Phase G, G1–G4).
- **Open:** G5, the parity bound, which is proposed after the first port stages are measured.
- **Not now:** GUI (E2), palette per material (E3). PIA solver stays as is (E5).

| Code | Topic | Status | Decision / Question |
|---|---|---|---|
| A1 | Language | Decided | Rust library plus `piaru` CLI. Spec. |
| A2 | Method | Decided | Pixelated Image Abstraction (Gerstner et al. 2012): superpixels are the output pixels, and segmentation and palette are solved jointly by mass-constrained deterministic annealing. Spec. |
| A3 | Colour space | Decided | Oklab ×100, so the paper's CIELAB-tuned constants keep their scale. CIELAB only for parity (`--space cielab`). Spec. |
| A4 | Reference | Decided | Port the objective, constants and loop from gskaggs/Pyxeled, and check parity against its output (`docs/PORTING.md`). Spec. |
| A5 | Compute location | Decided | No heavy jobs on the dev laptop (~15 GB RAM, no GPU). Run them remotely. Global user rule. |
| B1 | Pyxeled bugs | Decided | The five bugs in `docs/PORTING.md` stay fixed by default and are reproduced only under `--pyxeled-compat`, for the parity check (A4). User choice. |
| B2 | Float precision | Decided | `F = f32`. Parity ΔE vs Pyxeled is 1.60 (f64: 0.46), with similar output quality. Accepted by user. |
| B3 | Input downscale | Decided | Keep Lanczos to ≤ 4× the output grid (`--input-factor 4`, 0 = off). The benchmark is built on factor 4. Delegated to agent. |
| B4 | Hard palette limit | Decided | Keep: if annealing stops before K_max, the remaining pairs are merged and associated once more, so output never exceeds `-k`. `-k` is a hard contract. Delegated to agent. |
| B5 | Default saturation | Decided | Keep the paper's 1.1× a/b boost as the CLI default. The benchmark pins 1.0, so ΔE is unaffected. Delegated to agent. |
| B6 | Split behaviour | Decided | Leave as ported (splits come from the 1.5 perturbation, so K doubles each early T step). Any fix would be a solver change, which D1 rules out before the benchmark. Revisit as a new row after Phase D. Delegated to agent. |
| B7 | Palette under-use | Decided | Accept for now (at K = 16 only 9–13 entries are used). Same reason as B6. The benchmark's `colours used` column records it. Delegated to agent. |
| C1 | Importance map | Decided | Keep p(s) ∝ 0.1 + mean importance of the pixels in s (`importance_floor`). It runs only as the `pia_imp` ablation, and changing it would move benchmark numbers (D1). Delegated to agent. |
| C2 | Weight-in-cost (edge map) | Decided | Experimental, off unless `--edge-weight` is given. Colour term × (1 + 0.5·edge). Spec. Superseded by E1 (cut). |
| C3 | Benchmark design | Decided | Moved to `../PIXSOTA/DECISIONS.md` as F26 (research). Code kept so old citations resolve. |
| C4 | Weight-in-cost verdict | Decided | Moved to `../PIXSOTA/DECISIONS.md` as F27 (research). Code kept so old citations resolve. |
| D1 | Benchmark first | Decided | Moved to `../PIXSOTA/DECISIONS.md` as F28 (research). Code kept so old citations resolve. |
| D2 | Benchmark host | Decided | Moved to `../PIXSOTA/DECISIONS.md` as F29 (research). Code kept so old citations resolve. |
| D3 | Quality and speed targets | Decided | Moved to `../PIXSOTA/DECISIONS.md` as F30 (research). Code kept so old citations resolve. |
| D4 | Raters | Decided | Moved to `../PIXSOTA/DECISIONS.md` as F31 (research). Code kept so old citations resolve. |
| E1 | Edge weight | Decided | Cut: remove `--edge-weight` and its cost term (C2). Judged bad on the 2026-11-17 showcase, no objective gain over PIA. User choice. |
| E2 | GUI (egui) | Decided | Not now. Raise again as a new row if wanted. User choice. |
| E3 | Palette per material | Decided | Not now. Raise again as a new row if wanted. User choice. |
| E4 | rustfmt | Decided | Format the whole crate once in its own commit, then enforce `cargo fmt --check`. User choice. |
| E5 | PIA solver | Decided | Good as is. No solver changes planned; B6 and B7 stay accepted. User choice. |
| E6 | Importance map | Decided | Keep `--importance` (C1): it has good cases on the showcase. User choice. |
| E7 | Automatic importance map | Decided | Port spectral-residual saliency (Hou & Zhang 2007) into PIARU as `--importance auto`, matching the Python `spectral_residual` that produced the showcase maps. A map file still works. User choice. |
| E8 | Method line-up | Decided | Three methods: base PIA (the default), PIA with importance map (E6, E7), and PixelOE (G1, `--method pixeloe`). The edge-weight variant is gone (E1). User choice. |
| G1 | PixelOE in Rust | Decided | Port PixelOE (Apache-2.0, vendored at `refs/PixelOE` commit 7ce444b) to Rust in this repository: it is good but unoptimised (≈1.5 GB peak at 512 vs ≈0.13 GB for PIA). Keep its licence notice. User choice. |
| G2 | Port scope | Decided | Everything, in one phase: outline expansion, all downscale modes, sharpen, colour step (k-means and colour transfer) and dithering. User choice. |
| G3 | Parity check | Decided | Mean Oklab ΔE against `pixeloe.torch` output on the test images, under a fixed bound, like A4. The bound is G5. User choice. |
| G4 | Interface | Decided | Same binary: a library module plus `piaru --method pixeloe`. PIA stays the default, so existing commands are unchanged; `--stats` keeps its shape. User choice. |
| G5 | Parity bound | Inspect | Port the first stages, measure ΔE vs `pixeloe.torch` (f32 and float-order noise), then propose the bound. |
