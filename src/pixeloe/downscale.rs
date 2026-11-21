//! PixelOE downscale modes (`pixeloe/torch/downscale/*.py`) and the `F.interpolate` modes
//! `pixelize` falls back to, following torch's CPU kernels (align_corners = False, no
//! antialiasing).

use super::lab::{lab_to_rgb, rgb_to_lab};
use super::Img;

fn lower_median(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[(v.len() - 1) / 2]
}

/// PixelOE `contrast_downscale(img, patch)`: per patch, L is the centre pixel unless the patch
/// is skewed (median below the mean with a long bright tail -> darkest; the mirror case ->
/// brightest); a and b are the patch medians.
pub fn contrast(img: &Img, patch: usize) -> Img {
    let lab = rgb_to_lab(img);
    let (oh, ow) = (img.h / patch, img.w / patch);
    let mut out = Img::new(3, oh, ow);
    let mut buf = Vec::with_capacity(patch * patch);
    for py in 0..oh {
        for px in 0..ow {
            let gather = |c: usize, buf: &mut Vec<f32>| {
                buf.clear();
                for i in 0..patch {
                    for j in 0..patch {
                        buf.push(lab.at(c, py * patch + i, px * patch + j));
                    }
                }
            };
            gather(0, &mut buf);
            let mid = buf[buf.len() / 2];
            let mu = (buf.iter().map(|&v| v as f64).sum::<f64>() / buf.len() as f64) as f32;
            let mut s = buf.clone();
            let med = lower_median(&mut s);
            let (mini, maxi) = (s[0], s[s.len() - 1]);
            let l = if med < mu && (maxi - med) > (med - mini) {
                mini
            } else if med > mu && (maxi - med) < (med - mini) {
                maxi
            } else {
                mid
            };
            out.set(0, py, px, l);
            for c in 1..3 {
                gather(c, &mut buf);
                out.set(c, py, px, lower_median(&mut buf));
            }
        }
    }
    lab_to_rgb(&out)
}

/// torch `linspace(start, end, steps)` in f32 (CPU kernel: forward from `start` for the first
/// half, backward from `end` for the second).
pub fn linspace(start: f32, end: f32, steps: usize) -> Vec<f32> {
    if steps == 1 {
        return vec![start];
    }
    let step = (end - start) / (steps - 1) as f32;
    let half = steps / 2;
    (0..steps)
        .map(|i| {
            if i < half {
                start + step * i as f32
            } else {
                end - step * (steps - i - 1) as f32
            }
        })
        .collect()
}

/// One step of PixelOE `batched_kmeans_iter` for one batch item: assign each point to its
/// nearest centroid (first on ties, optional per-point distance weight) and move centroids to
/// their cluster means. Returns the largest centroid move.
pub fn kmeans_step(data: &[[f32; 3]], cents: &mut [[f32; 3]], weights: Option<&[f32]>) -> f32 {
    let k = cents.len();
    // f64 sums: torch sums pairwise, so a long naive f32 sum drifts away from upstream
    // (a constant 204/255 image over 512x512 gave a 203 centroid).
    let mut sums = vec![[0.0f64; 3]; k];
    let mut counts = vec![0usize; k];
    for (n, p) in data.iter().enumerate() {
        let lab = nearest(p, cents, weights.map(|w| w[n]));
        counts[lab] += 1;
        for c in 0..3 {
            sums[lab][c] += p[c] as f64;
        }
    }
    let mut diff = 0.0f32;
    for j in 0..k {
        if counts[j] > 0 {
            for c in 0..3 {
                let m = (sums[j][c] / counts[j] as f64) as f32;
                diff = diff.max((m - cents[j][c]).abs());
                cents[j][c] = m;
            }
        }
    }
    diff
}

/// Index of the nearest centroid (squared distance, optional weight, first on ties).
#[inline]
pub fn nearest(p: &[f32; 3], cents: &[[f32; 3]], weight: Option<f32>) -> usize {
    let mut best = f32::INFINITY;
    let mut bi = 0;
    for (j, c) in cents.iter().enumerate() {
        let mut d = [0.0f32; 3];
        for i in 0..3 {
            let t = p[i] - c[i];
            d[i] = t * t;
            if let Some(w) = weight {
                d[i] *= w;
            }
        }
        let d = d[0] + d[1] + d[2];
        if d < best {
            best = d;
            bi = j;
        }
    }
    bi
}

/// Relative gap below which two centroid totals count as tied (G6).
const TIE_REL: f64 = 1e-5;

/// PixelOE `k_centroid_downscale_torch(img, pixel_size, 2)`: 2-means per patch, then the
/// centroid with the smaller total squared distance to the patch's pixels.
///
/// Divergence (G6): when a patch splits evenly the totals are mathematically equal and torch's
/// pick depends on its float summation order. Here a tie (relative gap < 1e-5) goes to the
/// first centroid, the one initialised from the patch maximum.
pub fn k_centroid(img: &Img, pixel_size: usize, k: usize) -> Img {
    k_centroid_ties(img, pixel_size, k).0
}

/// [`k_centroid`] plus, per output pixel, whether the pick was a tie and the patch's centroids.
pub(crate) fn k_centroid_ties(
    img: &Img,
    pixel_size: usize,
    k: usize,
) -> (Img, Vec<bool>, Vec<Vec<[f32; 3]>>) {
    let (h, w) = (img.h, img.w);
    let (hd, wd) = (h / pixel_size, w / pixel_size);
    let kh = (h as f64 / hd as f64).ceil() as usize;
    let kw = (w as f64 / wd as f64).ceil() as usize;
    let (dh, dw) = (hd * kh - h, wd * kw - w);
    let (top, left) = (dh / 2, dw / 2);
    let px = |c: usize, y: usize, x: usize| {
        let yy = (y as isize - top as isize).clamp(0, h as isize - 1) as usize;
        let xx = (x as isize - left as isize).clamp(0, w as isize - 1) as usize;
        img.at(c, yy, xx)
    };
    let patches: Vec<Vec<[f32; 3]>> = (0..hd * wd)
        .map(|p| {
            let (py, pxx) = (p / wd, p % wd);
            let mut v = Vec::with_capacity(kh * kw);
            for i in 0..kh {
                for j in 0..kw {
                    let (y, x) = (py * kh + i, pxx * kw + j);
                    v.push([px(0, y, x), px(1, y, x), px(2, y, x)]);
                }
            }
            v
        })
        .collect();
    let interp = linspace(0.0, 1.0, k);
    let mut cents: Vec<Vec<[f32; 3]>> = patches
        .iter()
        .map(|v| {
            let mut mx = [f32::NEG_INFINITY; 3];
            let mut mn = [f32::INFINITY; 3];
            for p in v {
                for c in 0..3 {
                    mx[c] = mx[c].max(p[c]);
                    mn[c] = mn[c].min(p[c]);
                }
            }
            interp
                .iter()
                .map(|&t| [0, 1, 2].map(|c| t * mn[c] + (1.0 - t) * mx[c]))
                .collect()
        })
        .collect();
    let iters = (2 * (k as f64).sqrt() as usize).max(4);
    for _ in 0..iters {
        let mut diff = 0.0f32;
        for (v, c) in patches.iter().zip(cents.iter_mut()) {
            diff = diff.max(kmeans_step(v, c, None));
        }
        if diff < 1.0 / 256.0 {
            break;
        }
    }
    let mut out = Img::new(3, hd, wd);
    let mut ties = vec![false; hd * wd];
    for (p, (v, c)) in patches.iter().zip(&cents).enumerate() {
        let totals: Vec<f64> = c
            .iter()
            .map(|cc| {
                v.iter()
                    .map(|q| (0..3).map(|i| ((q[i] - cc[i]) as f64).powi(2)).sum::<f64>())
                    .sum()
            })
            .collect();
        let mut bi = 0;
        for j in 1..totals.len() {
            let gap = (totals[bi] - totals[j]) / totals[bi].max(totals[j]).max(f64::MIN_POSITIVE);
            if gap.abs() < TIE_REL {
                ties[p] = true;
            } else if totals[j] < totals[bi] {
                bi = j;
                ties[p] = false;
            }
        }
        for (ch, &v) in c[bi].iter().enumerate() {
            out.set(ch, p / wd, p % wd, v);
        }
    }
    (out, ties, cents)
}

/// Separable resampling, height pass first (`rows[o]` = (index, weight) taps), as Lanczos does
/// with two dense matrix products.
fn apply_axes(img: &Img, ry: &[Vec<(usize, f32)>], rx: &[Vec<(usize, f32)>]) -> Img {
    let (oh, ow) = (ry.len(), rx.len());
    {
        let mut t = Img::new(img.c, oh, img.w);
        for c in 0..img.c {
            for (o, taps) in ry.iter().enumerate() {
                for x in 0..img.w {
                    let mut s = 0.0f32;
                    for &(i, wv) in taps {
                        s += wv * img.at(c, i, x);
                    }
                    t.set(c, o, x, s);
                }
            }
        }
        let mut out = Img::new(img.c, oh, ow);
        for c in 0..img.c {
            for y in 0..oh {
                for (o, taps) in rx.iter().enumerate() {
                    let mut s = 0.0f32;
                    for &(i, wv) in taps {
                        s += t.at(c, y, i) * wv;
                    }
                    out.set(c, y, o, s);
                }
            }
        }
        out
    }
}

/// PixelOE Lanczos weights along one axis (`compute_weights_and_indices`, a = 3, support
/// scaling 1.5), in f32 like upstream.
fn lanczos_axis(
    in_size: usize,
    out_size: usize,
    a: f32,
    support_scaling: f64,
) -> Vec<Vec<(usize, f32)>> {
    let scale64 = out_size as f64 / in_size as f64;
    let scale = scale64 as f32;
    let eff_a = if scale64 < 1.0 {
        a as f64 * support_scaling / scale64
    } else {
        a as f64
    } as f32;
    let kern = |x: f32| -> f32 {
        let zero = x.abs() < 1e-7;
        let x = if zero { 1.0 } else { x };
        let px = std::f32::consts::PI * x;
        let k = if x.abs() < a {
            (px.sin() * (px / a).sin()) / (px * px / a)
        } else {
            0.0
        };
        if zero {
            1.0
        } else {
            k
        }
    };
    let coords: Vec<f32> = (0..out_size).map(|o| o as f32 / scale).collect();
    let right_max = coords
        .iter()
        .map(|c| c + eff_a + 0.5)
        .fold(f32::MIN, f32::max);
    let n_idx = right_max as usize + 1;
    coords
        .iter()
        .map(|&coord| {
            let left = coord - eff_a + 0.5;
            let right = coord + eff_a + 0.5;
            let mut ws: Vec<(usize, f32)> = (0..n_idx)
                .map(|i| {
                    let fi = i as f32;
                    let valid = fi >= left && fi < right && i < in_size;
                    (i, if valid { kern(fi - coord) } else { 0.0 })
                })
                .collect();
            let sum: f32 = ws.iter().map(|w| w.1).sum();
            for w in &mut ws {
                w.1 /= sum + 1e-7;
            }
            ws.retain(|w| w.1.abs() > 1e-7);
            ws
        })
        .collect()
}

/// PixelOE `lanczos_resize(img, (oh, ow))`: height first, then width, clamped to 0..1.
pub fn lanczos(img: &Img, oh: usize, ow: usize) -> Img {
    if img.h == oh && img.w == ow {
        return img.clone();
    }
    let ident = |n: usize| (0..n).map(|i| vec![(i, 1.0f32)]).collect::<Vec<_>>();
    let ry = if img.h != oh {
        lanczos_axis(img.h, oh, 3.0, 1.5)
    } else {
        ident(img.h)
    };
    let rx = if img.w != ow {
        lanczos_axis(img.w, ow, 3.0, 1.5)
    } else {
        ident(img.w)
    };
    apply_axes(img, &ry, &rx).map(|v| v.clamp(0.0, 1.0))
}

/// `F.interpolate` modes that `pixelize` passes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Interp {
    Nearest,
    NearestExact,
    Bilinear,
    Bicubic,
    Area,
}

fn nearest_axis(in_size: usize, out_size: usize, exact: bool) -> Vec<Vec<(usize, f32)>> {
    let scale = in_size as f32 / out_size as f32;
    (0..out_size)
        .map(|o| {
            let i = if out_size == in_size {
                o
            } else if !exact && out_size == 2 * in_size {
                o >> 1
            } else {
                let f = if exact {
                    (o as f32 + 0.5) * scale
                } else {
                    o as f32 * scale
                };
                (f.floor() as usize).min(in_size - 1)
            };
            vec![(i, 1.0)]
        })
        .collect()
}

fn bilinear_axis(in_size: usize, out_size: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = in_size as f32 / out_size as f32;
    (0..out_size)
        .map(|o| {
            let real = (scale * (o as f32 + 0.5) - 0.5).max(0.0);
            let i0 = real as usize;
            let i1 = if i0 < in_size - 1 { i0 + 1 } else { i0 };
            let l1 = real - i0 as f32;
            vec![(i0, 1.0 - l1), (i1, l1)]
        })
        .collect()
}

fn bicubic_axis(in_size: usize, out_size: usize) -> Vec<Vec<(usize, f32)>> {
    const A: f32 = -0.75;
    let c1 = |x: f32| ((A + 2.0) * x - (A + 3.0)) * x * x + 1.0;
    let c2 = |x: f32| ((A * x - 5.0 * A) * x + 8.0 * A) * x - 4.0 * A;
    let scale = in_size as f32 / out_size as f32;
    (0..out_size)
        .map(|o| {
            let real = scale * (o as f32 + 0.5) - 0.5;
            let idx = real.floor();
            let t = real - idx;
            let w = [c2(t + 1.0), c1(t), c1(1.0 - t), c2(2.0 - t)];
            (0..4)
                .map(|j| {
                    let i = (idx as isize - 1 + j as isize).clamp(0, in_size as isize - 1) as usize;
                    (i, w[j])
                })
                .collect()
        })
        .collect()
}

fn area_axis(in_size: usize, out_size: usize) -> Vec<(usize, usize)> {
    (0..out_size)
        .map(|o| {
            (
                o * in_size / out_size,
                ((o + 1) * in_size).div_ceil(out_size),
            )
        })
        .collect()
}

/// `F.interpolate(img, size=(oh, ow), mode=...)`.
pub fn interpolate(img: &Img, oh: usize, ow: usize, mode: Interp) -> Img {
    match mode {
        Interp::Area => {
            let (ay, ax) = (area_axis(img.h, oh), area_axis(img.w, ow));
            let mut out = Img::new(img.c, oh, ow);
            for c in 0..img.c {
                for (y, &(y0, y1)) in ay.iter().enumerate() {
                    for (x, &(x0, x1)) in ax.iter().enumerate() {
                        let mut s = 0.0f32;
                        for yy in y0..y1 {
                            for xx in x0..x1 {
                                s += img.at(c, yy, xx);
                            }
                        }
                        out.set(c, y, x, s / ((y1 - y0) * (x1 - x0)) as f32);
                    }
                }
            }
            out
        }
        _ => {
            let axis = |i: usize, o: usize| match mode {
                Interp::Nearest => nearest_axis(i, o, false),
                Interp::NearestExact => nearest_axis(i, o, true),
                Interp::Bilinear => bilinear_axis(i, o),
                Interp::Bicubic => bicubic_axis(i, o),
                Interp::Area => unreachable!(),
            };
            let (ry, rx) = (axis(img.h, oh), axis(img.w, ow));
            // torch evaluates sum_y wy * (sum_x wx * v): x inner.
            let mut out = Img::new(img.c, oh, ow);
            for c in 0..img.c {
                for (y, ty) in ry.iter().enumerate() {
                    for (x, tx) in rx.iter().enumerate() {
                        let mut s = 0.0f32;
                        for &(iy, wy) in ty {
                            let mut r = 0.0f32;
                            for &(ix, wx) in tx {
                                r += img.at(c, iy, ix) * wx;
                            }
                            s += r * wy;
                        }
                        out.set(c, y, x, s);
                    }
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixeloe::fixtures::{assert_close, img};

    #[test]
    fn downscale_modes_match() {
        let m = img("a_match_color");
        assert_close("a_down_contrast", &contrast(&m, 4), 1e-5);
        assert_close("a_down_lanczos", &lanczos(&m, 10, 12), 1e-5);
        for (name, mode) in [
            ("nearest", Interp::Nearest),
            ("nearest-exact", Interp::NearestExact),
            ("bilinear", Interp::Bilinear),
            ("bicubic", Interp::Bicubic),
            ("area", Interp::Area),
        ] {
            assert_close(
                &format!("a_down_{name}"),
                &interpolate(&m, 10, 12, mode),
                1e-5,
            );
        }
    }

    /// G6: exact on non-tied patches; on tied ones both sides must pick one of the centroids.
    #[test]
    fn k_centroid_matches() {
        let m = img("a_match_color");
        let want = img("a_down_k_centroid");
        let (got, ties, cents) = k_centroid_ties(&m, 4, 2);
        let n = got.h * got.w;
        let px = |im: &Img, i: usize| [0, 1, 2].map(|c| im.data[c * n + i]);
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|c| (a[c] - b[c]).abs() <= 1e-5);
        let mut n_ties = 0;
        for i in 0..n {
            if ties[i] {
                n_ties += 1;
                assert!(
                    cents[i].iter().any(|&c| close(c, px(&want, i))),
                    "tie {i}: reference is not a centroid"
                );
                // G6: Pix++ gives the tie to the first centroid.
                assert!(
                    close(px(&got, i), cents[i][0]),
                    "tie {i}: not the first centroid"
                );
            } else {
                assert!(close(px(&got, i), px(&want, i)), "pixel {i} differs");
            }
        }
        eprintln!("k_centroid: {} exact, {n_ties} ties", n - n_ties);
    }

    #[test]
    fn linspace_matches_torch() {
        assert_eq!(linspace(0.0, 1.0, 2), vec![0.0, 1.0]);
        let v = linspace(0.0, 1.0, 5);
        assert_eq!(v, vec![0.0, 0.25, 0.5, 0.75, 1.0]);
    }
}
