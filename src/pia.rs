//! Pixelated Image Abstraction (Gerstner, Thomas, Gooch et al., NPAR 2012 / C&G 2013).
//!
//! Superpixels ARE the output pixels. Each iteration:
//!   1. `refine_superpixels` – SLIC-style assignment of input pixels to the 3×3 neighbouring
//!      superpixels, cost = ‖I(x) − palette colour of s‖ + m·√(N/M)·‖x − pos_s‖;
//!      update positions/mean colours; Laplacian-smooth positions (0.4); bilateral-smooth colours.
//!   2. `associate_and_refine_palette` – MCDA step, fused into one pass over superpixels:
//!      p(c_k|s) ∝ p(c_k)·exp(−‖m_s − c_k‖/T);  p(c_k) = Σ_s p(c_k|s)p(s);
//!      c_k = Σ_s m_s p(c_k|s) p(s) / p(c_k).  No N×K matrix is ever stored.
//!   3. On palette convergence: T ← αT, and split clusters whose two sub-colours separated
//!      (until K_max), otherwise perturb along the principal axis.
//!
//! The objective and constants are ported from gskaggs/Pyxeled (see `docs/PORTING.md` for the
//! list of Pyxeled bugs that are fixed here, and `Params::pyxeled_compat` which keeps them).

use crate::color::{dist, Space, F};
use rayon::prelude::*;
use std::time::Instant;

#[derive(Clone, Debug, serde::Serialize)]
pub struct Params {
    pub out_w: usize,
    pub out_h: usize,
    /// Hard palette size limit.
    pub k_max: usize,
    pub space: Space,
    /// SLIC compactness m (paper: 45).
    pub m: F,
    pub t0: F,
    pub t_final: F,
    pub alpha: F,
    pub eps_palette: F,
    pub eps_cluster: F,
    /// Sub-cluster perturbation = this × first principal axis (Pyxeled: 1.5).
    pub perturb: F,
    /// Laplacian smoothing weight on superpixel centres (paper: 0.4).
    pub laplacian: F,
    /// Blend weight of the bilateral neighbourhood average on superpixel colours (Pyxeled: 0.5).
    pub bilateral: F,
    /// Final a/b chroma multiplier (paper/Pyxeled: 1.1). 1.0 = off.
    pub saturation: F,
    /// Safety cap on total iterations.
    pub max_iters: usize,
    /// Importance map: p(s) ∝ floor + mean importance of the pixels in s.
    pub importance_floor: F,
    /// EXPERIMENTAL weight-in-cost: colour term × (1 + λ·edge). Only used if an edge map is given.
    pub edge_lambda: F,
    /// Reproduce Pyxeled's quirks (corner-anchored grid, perturb bug, no final merge).
    pub pyxeled_compat: bool,
}

impl Params {
    pub fn new(out_w: usize, out_h: usize, k_max: usize) -> Self {
        Self {
            out_w,
            out_h,
            k_max,
            space: Space::Oklab,
            m: 45.0,
            t0: 35.0,
            t_final: 1.0,
            alpha: 0.7,
            eps_palette: 1.0,
            eps_cluster: 0.25,
            perturb: 1.5,
            laplacian: 0.4,
            bilateral: 0.5,
            saturation: 1.1,
            max_iters: 5000,
            importance_floor: 0.1,
            edge_lambda: 0.5,
            pyxeled_compat: false,
        }
    }
}

pub struct Input<'a> {
    pub width: usize,
    pub height: usize,
    /// sRGB 0..1, row-major.
    pub rgb: &'a [[F; 3]],
    /// Optional importance map, 0..1 per input pixel.
    pub importance: Option<&'a [F]>,
    /// Optional edge weight map, 0..1 per input pixel (experimental weight-in-cost).
    pub edge: Option<&'a [F]>,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Timings {
    pub setup_ms: f64,
    pub assign_ms: f64,
    pub accumulate_ms: f64,
    pub smooth_ms: f64,
    pub associate_ms: f64,
    pub expand_ms: f64,
    pub finalize_ms: f64,
    pub total_ms: f64,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Stats {
    pub iterations: usize,
    pub temperature_steps: usize,
    pub hit_max_iters: bool,
    pub palette_size: usize,
    pub input_w: usize,
    pub input_h: usize,
    pub timings: Timings,
    /// (iteration, T, K, palette change) — one entry per iteration.
    pub trace: Vec<(usize, F, usize, F)>,
}

pub struct Output {
    pub width: usize,
    pub height: usize,
    /// Palette index per output pixel, row-major.
    pub labels: Vec<u32>,
    /// Palette in working space (after saturation).
    pub palette: Vec<[F; 3]>,
    pub palette_srgb: Vec<[u8; 3]>,
    pub stats: Stats,
}

impl Output {
    pub fn rgb8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.labels.len() * 3);
        for &l in &self.labels {
            out.extend_from_slice(&self.palette_srgb[l as usize]);
        }
        out
    }
}

#[derive(Clone, Copy)]
enum Cluster {
    Pair(usize, usize),
    Single(usize),
}

struct Solver<'a> {
    p: &'a Params,
    w_in: usize,
    h_in: usize,
    lab: Vec<[F; 3]>,
    importance: Option<&'a [F]>,
    edge: Option<&'a [F]>,
    n: usize,
    spatial_w: F,
    // superpixel state (SoA)
    pos: Vec<[F; 2]>,
    orig_pos: Vec<[F; 2]>,
    sp_color: Vec<[F; 3]>,
    pal_color: Vec<[F; 3]>,
    ps: Vec<F>,
    sp_assign: Vec<u32>,
    // palette
    palette: Vec<[F; 3]>,
    probs: Vec<F>,
    clusters: Vec<Cluster>,
    k: usize,
    delta: [F; 3],
    t: F,
    tm: Timings,
}

const CHUNK: usize = 1024;

pub fn run(input: &Input, p: &Params) -> Output {
    let t_start = Instant::now();
    let mut s = Solver::new(input, p);
    s.tm.setup_ms = ms(t_start);

    let mut stats = Stats { input_w: input.width, input_h: input.height, ..Default::default() };
    let mut it = 0;
    while s.t > p.t_final {
        if it >= p.max_iters {
            stats.hit_max_iters = true;
            break;
        }
        it += 1;
        s.refine_superpixels();
        let change = s.associate_and_refine_palette();
        stats.trace.push((it, s.t, s.k, change));
        if change < p.eps_palette {
            s.t *= p.alpha;
            stats.temperature_steps += 1;
            if s.k < p.k_max {
                let t0 = Instant::now();
                s.expand();
                s.tm.expand_ms += ms(t0);
            }
        }
    }
    stats.iterations = it;

    let t0 = Instant::now();
    let out = s.finalize(stats);
    let mut out = out;
    out.stats.timings.finalize_ms += ms(t0);
    out.stats.timings.total_ms = ms(t_start);
    out
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

impl<'a> Solver<'a> {
    fn new(input: &Input<'a>, p: &'a Params) -> Self {
        let (w_in, h_in) = (input.width, input.height);
        let (w_out, h_out) = (p.out_w, p.out_h);
        assert!(w_out >= 1 && h_out >= 1 && w_out <= w_in && h_out <= h_in, "output must not exceed input");
        assert!(p.k_max >= 1);
        let m_px = w_in * h_in;
        let n = w_out * h_out;

        let lab: Vec<[F; 3]> = input.rgb.par_iter().map(|&c| p.space.from_srgb(c)).collect();

        // mean colour + first principal axis (power iteration on the 3×3 covariance)
        let (mean, cov) = mean_cov(&lab);
        let mut v = principal_axis(&cov);
        if p.pyxeled_compat {
            // sklearn (Pyxeled era) svd_flip(u_based_decision=True): the sample with the
            // largest |projection| projects positively.
            let proj = lab
                .par_iter()
                .map(|c| (c[0] - mean[0]) * v[0] + (c[1] - mean[1]) * v[1] + (c[2] - mean[2]) * v[2])
                .reduce(|| 0.0, |a: F, b: F| if b.abs() > a.abs() { b } else { a });
            if proj < 0.0 {
                v = v.map(|x| -x);
            }
        } else {
            let i = (0..3).max_by(|&a, &b| v[a].abs().total_cmp(&v[b].abs())).unwrap();
            if v[i] < 0.0 {
                v = v.map(|x| -x);
            }
        }
        let delta = v.map(|x| x * p.perturb);

        let mut pos = Vec::with_capacity(n);
        let mut orig_color = Vec::with_capacity(n);
        for r in 0..h_out {
            for c in 0..w_out {
                let (x, y) = if p.pyxeled_compat {
                    ((c * w_in / w_out) as F, (r * h_in / h_out) as F)
                } else {
                    // cell centres, with pixel centres at integer + 0.5
                    ((c as F + 0.5) * w_in as F / w_out as F, (r as F + 0.5) * h_in as F / h_out as F)
                };
                pos.push([x, y]);
                let (ix, iy) = ((x.floor() as usize).min(w_in - 1), (y.floor() as usize).min(h_in - 1));
                orig_color.push(lab[iy * w_in + ix]);
            }
        }

        let mut palette = vec![mean, mean];
        perturb(&mut palette[1], &delta, p.pyxeled_compat);

        Self {
            p,
            w_in,
            h_in,
            lab,
            importance: input.importance,
            edge: input.edge,
            n,
            spatial_w: p.m * ((n as F) / (m_px as F)).sqrt(),
            orig_pos: pos.clone(),
            pos,
            sp_color: orig_color,
            pal_color: vec![mean; n],
            ps: vec![1.0 / n as F; n],
            sp_assign: vec![0; n],
            palette,
            probs: vec![0.5, 0.5],
            clusters: vec![Cluster::Pair(0, 1)],
            k: 1,
            delta,
            t: p.t0,
            tm: Timings::default(),
        }
    }

    fn refine_superpixels(&mut self) {
        let (w_in, h_in) = (self.w_in, self.h_in);
        let (w_out, h_out) = (self.p.out_w, self.p.out_h);
        let off = if self.p.pyxeled_compat { 0.0 } else { 0.5 };
        let lambda = self.p.edge_lambda;

        // --- 1a+1b. assignment fused with accumulation, parallel over bands of input rows.
        // A band's pixels can only land in superpixel rows [gr(y0)-1, gr(y1)+1], so each band
        // keeps a small local accumulator for just those rows; bands are merged afterwards.
        let t0 = Instant::now();
        let n = self.n;
        let rows_per_band = (h_in / (rayon::current_num_threads() * 4)).max(1);
        let bands: Vec<(usize, Vec<[f64; 7]>)> = {
            let lab = &self.lab;
            let pos = &self.pos;
            let pal = &self.pal_color;
            let edge = self.edge;
            let imp = self.importance;
            let sw = self.spatial_w;
            (0..h_in.div_ceil(rows_per_band))
                .into_par_iter()
                .map(|b| {
                    let y0 = b * rows_per_band;
                    let y1 = (y0 + rows_per_band).min(h_in);
                    let r_lo = (y0 * h_out / h_in).saturating_sub(1);
                    let r_hi = ((y1 - 1) * h_out / h_in + 1).min(h_out - 1);
                    let base = r_lo * w_out;
                    // count, x, y, L, a, b, importance
                    let mut acc = vec![[0.0f64; 7]; (r_hi - r_lo + 1) * w_out];
                    for y in y0..y1 {
                        let gr = (y * h_out / h_in) as isize;
                        let fy = y as F + off;
                        for x in 0..w_in {
                            let gc = (x * w_out / w_in) as isize;
                            let fx = x as F + off;
                            let i = y * w_in + x;
                            let col = &lab[i];
                            let cmul = edge.map_or(1.0, |e| 1.0 + lambda * e[i]);
                            let mut best = F::INFINITY;
                            let mut best_s = 0usize;
                            for dc in -1isize..=1 {
                                let c = gc + dc;
                                if c < 0 || c >= w_out as isize {
                                    continue;
                                }
                                for dr in -1isize..=1 {
                                    let r = gr + dr;
                                    if r < 0 || r >= h_out as isize {
                                        continue;
                                    }
                                    let s = r as usize * w_out + c as usize;
                                    let ps = &pos[s];
                                    let dx = ps[0] - fx;
                                    let dy = ps[1] - fy;
                                    let cost = cmul * dist(col, &pal[s]) + sw * (dx * dx + dy * dy).sqrt();
                                    if cost < best {
                                        best = cost;
                                        best_s = s;
                                    }
                                }
                            }
                            let a = &mut acc[best_s - base];
                            a[0] += 1.0;
                            a[1] += fx as f64;
                            a[2] += fy as f64;
                            a[3] += col[0] as f64;
                            a[4] += col[1] as f64;
                            a[5] += col[2] as f64;
                            if let Some(imp) = imp {
                                a[6] += imp[i] as f64;
                            }
                        }
                    }
                    (base, acc)
                })
                .collect()
        };
        self.tm.assign_ms += ms(t0);

        let t0 = Instant::now();
        let mut acc = vec![[0.0f64; 7]; n];
        for (base, local) in &bands {
            for (j, a) in local.iter().enumerate() {
                let g = &mut acc[base + j];
                for q in 0..7 {
                    g[q] += a[q];
                }
            }
        }
        let mut ps_sum = 0.0;
        for (s, a) in acc.iter().enumerate() {
            if a[0] == 0.0 {
                self.pos[s] = self.orig_pos[s];
                // colour keeps its previous value (Pyxeled behaves the same, see PORTING.md)
            } else {
                let inv = 1.0 / a[0];
                self.pos[s] = [(a[1] * inv) as F, (a[2] * inv) as F];
                self.sp_color[s] = [(a[3] * inv) as F, (a[4] * inv) as F, (a[5] * inv) as F];
            }
            if self.importance.is_some() {
                let mean_imp = if a[0] > 0.0 { a[6] / a[0] } else { 0.0 };
                self.ps[s] = self.p.importance_floor + mean_imp as F;
                ps_sum += self.ps[s] as f64;
            }
        }
        if self.importance.is_some() {
            let inv = (1.0 / ps_sum) as F;
            self.ps.iter_mut().for_each(|v| *v *= inv);
        }
        self.tm.accumulate_ms += ms(t0);

        // --- 1c. Laplacian smoothing of positions, bilateral smoothing of colours
        let t0 = Instant::now();
        let lap = self.p.laplacian;
        let bil = self.p.bilateral;
        let pos = &self.pos;
        let col = &self.sp_color;
        let (new_pos, new_col): (Vec<[F; 2]>, Vec<[F; 3]>) = (0..n)
            .into_par_iter()
            .map(|s| {
                let (r, c) = ((s / w_out) as isize, (s % w_out) as isize);
                let inb = |r: isize, c: isize| r >= 0 && c >= 0 && r < h_out as isize && c < w_out as isize;
                let mut sx = 0.0;
                let mut sy = 0.0;
                let mut cnt = 0.0;
                for (dr, dc) in [(0, 1), (0, -1), (-1, 0), (1, 0)] {
                    if inb(r + dr, c + dc) {
                        let q = &pos[((r + dr) as usize) * w_out + (c + dc) as usize];
                        sx += q[0];
                        sy += q[1];
                        cnt += 1.0;
                    }
                }
                let me = &pos[s];
                let np = if cnt > 0.0 {
                    [lap * sx / cnt + (1.0 - lap) * me[0], lap * sy / cnt + (1.0 - lap) * me[1]]
                } else {
                    *me
                };

                let mc = &col[s];
                let mut acc = [0.0; 3];
                let mut wsum = 0.0;
                for dr in -1isize..=1 {
                    for dc in -1isize..=1 {
                        if (dr == 0 && dc == 0) || !inb(r + dr, c + dc) {
                            continue;
                        }
                        let q = &col[((r + dr) as usize) * w_out + (c + dc) as usize];
                        let w = (-(mc[0] - q[0]).abs()).exp();
                        acc[0] += w * q[0];
                        acc[1] += w * q[1];
                        acc[2] += w * q[2];
                        wsum += w;
                    }
                }
                let nc = if wsum > 0.0 {
                    [
                        (1.0 - bil) * mc[0] + bil * acc[0] / wsum,
                        (1.0 - bil) * mc[1] + bil * acc[1] / wsum,
                        (1.0 - bil) * mc[2] + bil * acc[2] / wsum,
                    ]
                } else {
                    *mc
                };
                (np, nc)
            })
            .unzip();
        self.pos = new_pos;
        self.sp_color = new_col;
        self.tm.smooth_ms += ms(t0);
    }

    /// Fused MCDA association + palette refinement. Returns Σ_k ‖Δc_k‖.
    fn associate_and_refine_palette(&mut self) -> F {
        let t0 = Instant::now();
        let kn = self.palette.len();
        let palette = &self.palette;
        let probs = &self.probs;
        let col = &self.sp_color;
        let ps = &self.ps;
        let inv_t = 1.0 / self.t;

        // acc layout per palette entry: [mass, L, a, b], in f64 for stable summation
        let acc = self
            .sp_assign
            .par_chunks_mut(CHUNK)
            .enumerate()
            .map(|(ci, chunk)| {
                let mut acc = vec![0.0f64; kn * 4];
                let mut d = vec![0.0 as F; kn];
                let mut w = vec![0.0 as F; kn];
                for (j, out) in chunk.iter_mut().enumerate() {
                    let s = ci * CHUNK + j;
                    let m = &col[s];
                    let mut dmin = F::INFINITY;
                    for k in 0..kn {
                        d[k] = dist(m, &palette[k]);
                        dmin = dmin.min(d[k]);
                    }
                    // exp(-(d - dmin)/T) is the same distribution, without f32 underflow at low T
                    let mut z = 0.0;
                    let mut best = 0usize;
                    let mut bw = -1.0;
                    for k in 0..kn {
                        w[k] = probs[k] * (-(d[k] - dmin) * inv_t).exp();
                        z += w[k];
                        if w[k] > bw {
                            bw = w[k];
                            best = k;
                        }
                    }
                    *out = best as u32;
                    let scale = ps[s] as f64 / z as f64;
                    for k in 0..kn {
                        let pk = w[k] as f64 * scale;
                        let a = &mut acc[k * 4..k * 4 + 4];
                        a[0] += pk;
                        a[1] += pk * m[0] as f64;
                        a[2] += pk * m[1] as f64;
                        a[3] += pk * m[2] as f64;
                    }
                }
                acc
            })
            .reduce(
                || vec![0.0f64; kn * 4],
                |mut a, b| {
                    a.iter_mut().zip(b).for_each(|(x, y)| *x += y);
                    a
                },
            );

        // superpixels take the (pre-refinement) colour of their most probable palette entry
        let palette_now = self.palette.clone();
        self.pal_color.par_iter_mut().zip(self.sp_assign.par_iter()).for_each(|(pc, &k)| *pc = palette_now[k as usize]);

        let mut change = 0.0;
        for k in 0..kn {
            let mass = acc[k * 4];
            self.probs[k] = mass as F;
            if mass > 1e-12 {
                let nc = [(acc[k * 4 + 1] / mass) as F, (acc[k * 4 + 2] / mass) as F, (acc[k * 4 + 3] / mass) as F];
                change += dist(&self.palette[k], &nc);
                self.palette[k] = nc;
            }
        }
        self.tm.associate_ms += ms(t0);
        change
    }

    /// Port of Pyxeled `expand()`: split separated sub-clusters, merge everything at K_max,
    /// otherwise perturb the second member of each pair.
    fn expand(&mut self) {
        let k_max = self.p.k_max;
        let k0 = self.k;
        for i in 0..k0 {
            if self.k >= k_max {
                break;
            }
            let Cluster::Pair(a, b) = self.clusters[i] else { continue };
            if dist(&self.palette[a], &self.palette[b]) > self.p.eps_cluster {
                self.k += 1;
                let (ca, cb) = (self.palette[a], self.palette[b]);
                let (pa, pb) = (self.probs[a] / 2.0, self.probs[b] / 2.0);
                self.palette.push(ca);
                self.probs.push(pa);
                self.palette.push(cb);
                self.probs.push(pb);
                self.probs[a] = pa;
                self.probs[b] = pb;
                let l = self.palette.len();
                self.clusters.push(Cluster::Pair(b, l - 1));
                self.clusters[i] = Cluster::Pair(a, l - 2);
            }
        }
        if self.k >= k_max {
            self.merge_pairs();
        } else {
            for i in 0..self.k {
                if let Cluster::Pair(_, b) = self.clusters[i] {
                    let d = self.delta;
                    perturb(&mut self.palette[b], &d, self.p.pyxeled_compat);
                }
            }
        }
    }

    fn merge_pairs(&mut self) {
        let mut pal = Vec::with_capacity(self.k);
        let mut pr = Vec::with_capacity(self.k);
        for c in &self.clusters[..self.k] {
            match *c {
                Cluster::Pair(a, b) => {
                    let (ca, cb) = (self.palette[a], self.palette[b]);
                    pal.push([(ca[0] + cb[0]) / 2.0, (ca[1] + cb[1]) / 2.0, (ca[2] + cb[2]) / 2.0]);
                    pr.push(self.probs[a] + self.probs[b]);
                }
                Cluster::Single(a) => {
                    pal.push(self.palette[a]);
                    pr.push(self.probs[a]);
                }
            }
        }
        self.clusters = (0..pal.len()).map(Cluster::Single).collect();
        self.palette = pal;
        self.probs = pr;
    }

    fn finalize(mut self, mut stats: Stats) -> Output {
        let (palette, labels) = if self.p.pyxeled_compat {
            // Pyxeled writes each superpixel's last pre-refinement palette colour.
            let mut uniq: Vec<[F; 3]> = Vec::new();
            let labels = self
                .pal_color
                .iter()
                .map(|c| {
                    let i = uniq.iter().position(|u| u == c).unwrap_or_else(|| {
                        uniq.push(*c);
                        uniq.len() - 1
                    });
                    i as u32
                })
                .collect();
            (uniq, labels)
        } else {
            // Hard constraint: collapse any unsplit pairs, then one last association at the
            // final temperature against the final palette.
            if self.clusters.iter().any(|c| matches!(c, Cluster::Pair(..))) {
                self.merge_pairs();
            }
            self.t = self.t.max(1e-6);
            self.associate_and_refine_palette_assign_only();
            (self.palette.clone(), self.sp_assign.clone())
        };

        let sat = self.p.saturation;
        let palette: Vec<[F; 3]> = palette.iter().map(|c| [c[0], c[1] * sat, c[2] * sat]).collect();
        let palette_srgb = palette
            .iter()
            .map(|&c| self.p.space.to_srgb(c).map(|v| (v * 255.0).round() as u8))
            .collect::<Vec<_>>();
        stats.palette_size = palette.len();
        stats.timings = self.tm.clone();
        Output { width: self.p.out_w, height: self.p.out_h, labels, palette, palette_srgb, stats }
    }

    /// Final assignment only: argmax_k p(c_k)·exp(−‖m_s − c_k‖/T).
    fn associate_and_refine_palette_assign_only(&mut self) {
        let palette = &self.palette;
        let probs = &self.probs;
        let col = &self.sp_color;
        let inv_t = 1.0 / self.t;
        self.sp_assign.par_iter_mut().enumerate().for_each(|(s, out)| {
            let m = &col[s];
            // argmax p_k exp(-d_k/T)  ==  argmin d_k/T - ln p_k
            let mut best = 0;
            let mut bv = F::INFINITY;
            for k in 0..palette.len() {
                let v = dist(m, &palette[k]) * inv_t - probs[k].max(1e-30).ln();
                if v < bv {
                    bv = v;
                    best = k;
                }
            }
            *out = best as u32;
        });
    }
}

fn perturb(c: &mut [F; 3], d: &[F; 3], compat: bool) {
    c[0] += d[0];
    c[1] += d[1];
    // Pyxeled bug: perturbs the third channel by delta[1]
    c[2] += if compat { d[1] } else { d[2] };
}

fn mean_cov(lab: &[[F; 3]]) -> ([F; 3], [[f64; 3]; 3]) {
    let n = lab.len() as f64;
    let sum = lab
        .par_iter()
        .fold(|| [0.0f64; 3], |mut a, c| {
            for i in 0..3 {
                a[i] += c[i] as f64;
            }
            a
        })
        .reduce(|| [0.0; 3], |a, b| [a[0] + b[0], a[1] + b[1], a[2] + b[2]]);
    let mean = sum.map(|v| v / n);
    let cov = lab
        .par_iter()
        .fold(|| [[0.0f64; 3]; 3], |mut a, c| {
            let d = [c[0] as f64 - mean[0], c[1] as f64 - mean[1], c[2] as f64 - mean[2]];
            for i in 0..3 {
                for j in 0..3 {
                    a[i][j] += d[i] * d[j];
                }
            }
            a
        })
        .reduce(
            || [[0.0; 3]; 3],
            |mut a, b| {
                for i in 0..3 {
                    for j in 0..3 {
                        a[i][j] += b[i][j];
                    }
                }
                a
            },
        );
    let cov = cov.map(|r| r.map(|v| v / (n - 1.0).max(1.0)));
    (mean.map(|v| v as F), cov)
}

fn principal_axis(cov: &[[f64; 3]; 3]) -> [F; 3] {
    let mut v = [0.577_350_3f64, 0.577_350_3, 0.577_350_3];
    for _ in 0..200 {
        let nv = [
            cov[0][0] * v[0] + cov[0][1] * v[1] + cov[0][2] * v[2],
            cov[1][0] * v[0] + cov[1][1] * v[1] + cov[1][2] * v[2],
            cov[2][0] * v[0] + cov[2][1] * v[1] + cov[2][2] * v[2],
        ];
        let norm = (nv[0] * nv[0] + nv[1] * nv[1] + nv[2] * nv[2]).sqrt();
        if norm < 1e-20 {
            return [1.0, 0.0, 0.0]; // flat image
        }
        v = nv.map(|x| x / norm);
    }
    v.map(|x| x as F)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(w: usize, h: usize) -> Vec<[F; 3]> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as F / w as F, (i / w) as F / h as F);
                if x < 0.5 {
                    [0.9, 0.2 + 0.1 * y, 0.1]
                } else if y < 0.5 {
                    [0.1, 0.3, 0.8]
                } else {
                    [0.95, 0.95, 0.9]
                }
            })
            .collect()
    }

    #[test]
    fn respects_hard_palette_limit_and_finds_regions() {
        let (w, h) = (64, 48);
        let rgb = synthetic(w, h);
        let input = Input { width: w, height: h, rgb: &rgb, importance: None, edge: None };
        let mut p = Params::new(16, 12, 4);
        p.saturation = 1.0;
        let out = run(&input, &p);
        assert!(out.palette.len() <= 4);
        assert_eq!(out.labels.len(), 16 * 12);
        let mut used: Vec<u32> = out.labels.clone();
        used.sort();
        used.dedup();
        assert!(used.len() >= 3, "three flat regions should get three colours, got {}", used.len());
        assert!(!out.stats.hit_max_iters);
    }
}
