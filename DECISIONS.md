# Decisions

Source of truth: overrules `ARCHITECTURE.md` and `ROADMAP.md`. A code is the roadmap phase letter plus a sequence number. Codes are stable and never reused, and a new feature takes the next free letter. A phase starts only when its rows are Decided.

**Status:** **Decided**: binding, with the reason in a few words. **Inspect**: research or a spike first. **TBD**: ask, don't guess. **Question**: not yet read by the human, so treat it as TBD.

**Phases:** A Foundations (done), B Core solver and parity (done), C Controls and harness (done), D Benchmark run and report, E Post-benchmark features.

**Scope:** this file covers the implementation only. The research project (paper, literature review, benchmark harness, human study) lives in the sibling `../PIXSOTA` checkout with its own `DECISIONS.md` (F rows). Rows `D1`–`D4` and `C3`/`C4` below are inputs to that project and must stay here.

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
| C2 | Weight-in-cost (edge map) | Decided | Experimental, off unless `--edge-weight` is given. Colour term × (1 + 0.5·edge). Spec. |
| C3 | Benchmark design | Decided | 4 columns: PixelOE + Oklab k-means, pia, pia_imp, pia_edge. 8 images × 128/256/512, K = 16, saturation 1.0. pia_imp uses spectral-residual saliency instead of a hand-painted map. Accepted by user. |
| C4 | Weight-in-cost verdict | Decided | Keep pia_edge only if it wins ≥ 60 % of non-tied blind (trial, rater) pairs and its mean score is ≥ 0.5 higher. Otherwise cut it (verdict computed by `../PIXSOTA/bench/score.py`). Spec. |
| D1 | Benchmark first | Decided | Report benchmark numbers before any feature work. Spec. |
| D2 | Benchmark host | Decided | `asus` over ssh, inside WSL (kali, 12 threads, 15 GB), via `../PIXSOTA/bench/remote_wsl.sh`. User choice. |
| D3 | Quality and speed targets | Decided | No numeric targets. Quality is judged by the user's blind rating. Cost is reported as peak memory (RSS) and inference time (solve ms) per size. User choice. |
| D4 | Raters | Decided | The user is the only rater. User choice. |
| E1 | Weight-in-cost after verdict | TBD | After C4: cut the code, or keep it as an opt-in flag? |
| E2 | GUI (egui) | TBD | Wanted? In what scope? |
| E3 | Palette per material | TBD | Wanted? In what scope? |
| E4 | rustfmt | TBD | `cargo fmt --check` reports diffs. Format once and enforce, or leave? |
