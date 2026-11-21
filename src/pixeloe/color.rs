//! PixelOE colour step (`pixeloe/torch/color.py`): Lab statistics transfer plus wavelet colour
//! fix (`match_color`), k-means palette (`color_quantization_kmeans`) and dithering.

use super::downscale::{kmeans_step, linspace, nearest};
use super::lab::{lab_to_rgb, rgb_to_lab};
use super::Img;
use rayon::prelude::*;

/// Round an f32 to the nearest IEEE half (ties to even) and back, as torch's float16 ops do.
pub fn round_f16(x: f32) -> f32 {
    let b = x.to_bits();
    let sign = b & 0x8000_0000;
    let abs = b & 0x7fff_ffff;
    if abs >= 0x7f80_0000 {
        return x; // inf / nan
    }
    let a = f32::from_bits(abs);
    // Largest finite half is 65504; anything that rounds above it becomes inf.
    if a >= 65520.0 {
        return f32::from_bits(sign | 0x7f80_0000);
    }
    let e = ((abs >> 23) as i32) - 127;
    // Spacing of halves around `a`: 2^(e-10) for normals, 2^-24 for subnormals.
    let ulp_exp = if e < -14 { -24 } else { e - 10 };
    let ulp = f32::from_bits(((ulp_exp + 127) as u32) << 23);
    let q = a / ulp; // exact: power-of-two scaling
    let r = q.round_ties_even();
    f32::from_bits(sign | (r * ulp).to_bits())
}

/// PixelOE `gaussian_kernel(radius)`, built in float16 like upstream, returned as f32.
pub fn gaussian_kernel(radius: usize) -> Img {
    let n = 2 * radius + 1;
    let denom = (2 * radius * radius) as f32;
    let k1: Vec<f32> = (0..n)
        .map(|i| {
            let x = i as f32 - radius as f32;
            let x2 = round_f16(x * x);
            round_f16(round_f16(-x2 / denom).exp())
        })
        .collect();
    let mut k2 = vec![0.0f32; n * n];
    for i in 0..n {
        for j in 0..n {
            k2[i * n + j] = round_f16(k1[i] * k1[j]);
        }
    }
    let sum = round_f16(k2.iter().map(|&v| v as f64).sum::<f64>() as f32);
    Img::from_vec(1, n, n, k2.iter().map(|&v| round_f16(v / sum)).collect())
}

/// PixelOE `wavelet_blur(x, radius)`: depthwise convolution with [`gaussian_kernel`], reflect
/// padding (replicate when the image is not larger than the radius).
pub fn wavelet_blur(img: &Img, radius: usize) -> Img {
    let k = gaussian_kernel(radius);
    let n = k.h;
    let reflect = img.h > radius && img.w > radius;
    let idx = |i: isize, len: usize| -> usize {
        let len = len as isize;
        let i = if reflect {
            if i < 0 {
                -i
            } else if i >= len {
                2 * (len - 1) - i
            } else {
                i
            }
        } else {
            i.clamp(0, len - 1)
        };
        i as usize
    };
    let (h, w) = (img.h, img.w);
    let ys: Vec<Vec<usize>> = (0..h)
        .map(|y| {
            (0..n)
                .map(|i| idx(y as isize + i as isize - radius as isize, h))
                .collect()
        })
        .collect();
    // Each source row padded horizontally once, so tap j of output x reads padded[x + j].
    let padded: Vec<f32> = (0..img.c * h)
        .flat_map(|row| {
            let src = &img.data[row * w..(row + 1) * w];
            (0..w + n - 1).map(move |p| src[idx(p as isize - radius as isize, w)])
        })
        .collect();
    let pw = w + n - 1;
    let mut out = Img::new(img.c, h, w);
    // Same per-pixel summation order as a direct loop (rows i, then columns j), so the result
    // is bit-identical; the x loop is innermost so it vectorises (G11).
    out.data
        .par_chunks_mut(w)
        .enumerate()
        .for_each(|(row, line)| {
            let (c, y) = (row / h, row % h);
            for (i, &yy) in ys[y].iter().enumerate() {
                let prow = &padded[(c * h + yy) * pw..(c * h + yy + 1) * pw];
                for (j, &kv) in k.data[i * n..(i + 1) * n].iter().enumerate() {
                    for (o, &v) in line.iter_mut().zip(&prow[j..j + w]) {
                        *o += v * kv;
                    }
                }
            }
        });
    out
}

/// PixelOE `wavelet_colorfix(inp, target, level)`: high frequencies of `inp` over the low
/// frequencies of `target`.
pub fn wavelet_colorfix(inp: &Img, target: &Img, level: u32) -> Img {
    let mut high = vec![0.0f32; inp.data.len()];
    let mut x = inp.clone();
    for i in 1..=level {
        let low = wavelet_blur(&x, 1 << i);
        for (h, (a, b)) in high.iter_mut().zip(x.data.iter().zip(&low.data)) {
            *h += a - b;
        }
        x = low;
    }
    let mut t = target.clone();
    for i in 1..=level {
        t = wavelet_blur(&t, 1 << i);
    }
    Img::from_vec(
        inp.c,
        inp.h,
        inp.w,
        high.iter().zip(&t.data).map(|(h, l)| h + l).collect(),
    )
}

/// Mean and unbiased standard deviation over every element (torch `mean` / `std` of the tensor).
fn mean_std(v: &[f32]) -> (f32, f32) {
    let n = v.len() as f64;
    let m = v.iter().map(|&x| x as f64).sum::<f64>() / n;
    let var = v.iter().map(|&x| (x as f64 - m).powi(2)).sum::<f64>() / (n - 1.0);
    (m as f32, var.sqrt() as f32)
}

/// PixelOE `match_color(source, target, level=5)`.
pub fn match_color(source: &Img, target: &Img) -> Img {
    let sl = rgb_to_lab(source);
    let tl = rgb_to_lab(target);
    let (ms, ss) = mean_std(&sl.data);
    let (mt, st) = mean_std(&tl.data);
    // A flat source (an all-black image) has no spread to scale; upstream divides by zero and
    // returns NaN. Shift it to the target's mean instead (docs/PORTING.md).
    let res = if ss > 0.0 {
        sl.map(|v| (v - ms) / ss * st + mt)
    } else {
        sl.map(|v| v - ms + mt)
    };
    wavelet_colorfix(&lab_to_rgb(&res), target, 5)
}

/// PixelOE `centroid_generator(maxv, minv, k)`.
fn initial_centroids(maxv: [f32; 3], minv: [f32; 3], k: usize) -> Vec<[f32; 3]> {
    let mix = |t: [f32; 3]| [0, 1, 2].map(|c| t[c] * minv[c] + (1.0 - t[c]) * maxv[c]);
    if k < 8 {
        return linspace(0.0, 1.0, k)
            .into_iter()
            .map(|t| mix([t; 3]))
            .collect();
    }
    let base = k / 4;
    let cent = k - base * 3;
    let mut out = Vec::with_capacity(k);
    for &t in &linspace(0.0, 1.0, base + 1)[1..] {
        for c in 0..3 {
            let mut v = [0.0f32; 3];
            v[c] = t;
            out.push(mix(v));
        }
    }
    for t in linspace(0.0, 1.0, cent) {
        out.push(mix([t; 3]));
    }
    out
}

/// PixelOE `generate_repeat_table(weights, N, 4N)`: how many times each pixel is repeated so
/// that the repeated set has 4N points, at least one each, distributed by weight. Ties in the
/// remainders go to the lower index (torch's `topk` order is unspecified there).
fn repeat_table(weights: &[f32], n2: usize) -> Vec<usize> {
    let n = weights.len();
    let logw: Vec<f64> = weights.iter().map(|&w| (w.max(1e-8) as f64).ln()).collect();
    let mx = logw.iter().cloned().fold(f64::MIN, f64::max);
    let lse = mx + logw.iter().map(|l| (l - mx).exp()).sum::<f64>().ln();
    let rem_n = (n2 - n) as f64;
    let mut counts = vec![1usize; n];
    let mut rems = Vec::with_capacity(n);
    let mut floor_sum = 0usize;
    for (i, l) in logw.iter().enumerate() {
        let r = ((l - lse).exp() as f32) as f64 * rem_n;
        let r = r as f32;
        let f = r.floor();
        counts[i] += f as usize;
        floor_sum += f as usize;
        rems.push((r - f, i));
    }
    let extra = (n2 - n).saturating_sub(floor_sum);
    rems.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    for &(_, i) in rems.iter().take(extra) {
        counts[i] += 1;
    }
    counts
}

/// PixelOE `color_quantization_kmeans`: returns the palette.
pub fn kmeans_palette(img: &Img, k: usize, weights: Option<&[f32]>, repeat: bool) -> Vec<[f32; 3]> {
    let n = img.h * img.w;
    let pixels: Vec<[f32; 3]> = (0..n)
        .map(|i| [img.data[i], img.data[n + i], img.data[2 * n + i]])
        .collect();
    let mut maxv = [f32::NEG_INFINITY; 3];
    let mut minv = [f32::INFINITY; 3];
    for p in &pixels {
        for c in 0..3 {
            maxv[c] = maxv[c].max(p[c]);
            minv[c] = minv[c].min(p[c]);
        }
    }
    let mut cents = initial_centroids(maxv, minv, k);
    let (inputs, w): (Vec<[f32; 3]>, Option<&[f32]>) = match (repeat, weights) {
        (true, Some(wt)) => {
            let counts = repeat_table(wt, n * 4);
            let rep = pixels
                .iter()
                .zip(&counts)
                .flat_map(|(p, &c)| std::iter::repeat_n(*p, c))
                .collect();
            (rep, None)
        }
        _ => (pixels.clone(), weights),
    };
    for _ in 0..2 * (k as f64).sqrt() as usize {
        if kmeans_step(&inputs, &mut cents, w) < 1.0 / 256.0 {
            break;
        }
    }
    cents
}

/// Nearest palette colour for every pixel.
fn apply_palette(img: &Img, pal: &[[f32; 3]]) -> Img {
    let n = img.h * img.w;
    let mut out = Img::new(3, img.h, img.w);
    for i in 0..n {
        let p = [img.data[i], img.data[n + i], img.data[2 * n + i]];
        let q = pal[nearest(&p, pal, None)];
        for (c, &v) in q.iter().enumerate() {
            out.data[c * n + i] = v;
        }
    }
    out
}

/// Bayer matrix of size `n` (power of two), values / n^2.
fn bayer(n: usize) -> Vec<Vec<f32>> {
    fn rec(n: usize) -> Vec<Vec<u32>> {
        if n == 2 {
            return vec![vec![0, 2], vec![3, 1]];
        }
        let s = rec(n / 2);
        let h = n / 2;
        let mut m = vec![vec![0u32; n]; n];
        for i in 0..h {
            for j in 0..h {
                m[i][j] = 4 * s[i][j];
                m[i][j + h] = 4 * s[i][j] + 2;
                m[i + h][j] = 4 * s[i][j] + 3;
                m[i + h][j + h] = 4 * s[i][j] + 1;
            }
        }
        m
    }
    let d = (n * n) as f32;
    rec(n)
        .into_iter()
        .map(|r| r.into_iter().map(|v| v as f32 / d).collect())
        .collect()
}

/// PixelOE `ordered_dither`: 8x8 Bayer threshold between the two nearest palette colours.
fn ordered_dither(img: &Img, pal: &[[f32; 3]]) -> Img {
    let b = bayer(8);
    let n = img.h * img.w;
    let mut out = Img::new(3, img.h, img.w);
    for y in 0..img.h {
        for x in 0..img.w {
            let i = y * img.w + x;
            let p = [img.data[i], img.data[n + i], img.data[2 * n + i]];
            // The two nearest colours in one scan, ordered by (distance, index) as a full sort
            // would order them.
            let mut first: Option<(f32, usize)> = None;
            let mut second: Option<(f32, usize)> = None;
            let before = |a: (f32, usize), b: Option<(f32, usize)>| {
                b.is_none_or(|b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).is_lt())
            };
            for (j, q) in pal.iter().enumerate() {
                let t = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                let c = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2], j);
                if before(c, first) {
                    second = first;
                    first = Some(c);
                } else if before(c, second) {
                    second = Some(c);
                }
            }
            let d0 = first.expect("palette has at least one colour");
            let Some(d1) = second else {
                for (c, &v) in pal[d0.1].iter().enumerate() {
                    out.data[c * n + i] = v;
                }
                continue;
            };
            let ratio = d0.0 / (d0.0 + d1.0 + 1e-6);
            let pick = if b[y % 8][x % 8] > ratio { d0.1 } else { d1.1 };
            for (c, &v) in pal[pick].iter().enumerate() {
                out.data[c * n + i] = v;
            }
        }
    }
    out
}

/// PixelOE `parallel_error_diffusion`: two rows at a time, error spread with the kernel
/// [[0, 0, 7], [3, 5, 1]] / 16 over reflect-padded error. Only the rows that feed the update
/// are recomputed; each pixel's error depends only on its own value, so this is the same.
fn error_diffusion(img: &Img, pal: &[[f32; 3]]) -> Img {
    let (h, w) = (img.h, img.w);
    let n = h * w;
    let k = [[0.0f32, 0.0, 7.0], [3.0, 5.0, 1.0]].map(|r| r.map(|v| v / 16.0));
    let mut out = img.clone();
    let refl = |i: isize, len: usize| -> usize {
        if len == 1 {
            return 0;
        }
        let len = len as isize;
        (if i < 0 {
            -i
        } else if i >= len {
            2 * (len - 1) - i
        } else {
            i
        }) as usize
    };
    let mut y = 0;
    while y < h {
        if y + 2 < h {
            // error of rows y .. y+2 (the reflect-padded rows the update reads)
            let err_at = |out: &Img, c: usize, yy: usize, xx: usize| -> f32 {
                let i = yy * w + xx;
                let p = [out.data[i], out.data[n + i], out.data[2 * n + i]];
                out.data[c * n + i] - pal[nearest(&p, pal, None)][c]
            };
            let mut upd = vec![0.0f32; 3 * 2 * w];
            for c in 0..3 {
                for (r, row) in (y + 1..y + 3).enumerate() {
                    for x in 0..w {
                        let mut s = 0.0f32;
                        for (a, kr) in k.iter().enumerate() {
                            let yy = refl(row as isize + a as isize - 1, h);
                            for (b, &kv) in kr.iter().enumerate() {
                                let xx = refl(x as isize + b as isize - 1, w);
                                s += err_at(&out, c, yy, xx) * kv;
                            }
                        }
                        upd[(c * 2 + r) * w + x] = s;
                    }
                }
            }
            for c in 0..3 {
                for r in 0..2 {
                    for x in 0..w {
                        out.data[c * n + (y + 1 + r) * w + x] += upd[(c * 2 + r) * w + x];
                    }
                }
            }
        }
        y += 2;
    }
    apply_palette(&out, pal)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dither {
    Ordered,
    ErrorDiffusion,
    None,
}

/// PixelOE `quantize_and_dither` (k-means palette, then dithering with it).
pub fn quantize_and_dither(
    img: &Img,
    weights: Option<&[f32]>,
    k: usize,
    dither: Dither,
    repeat: bool,
) -> Img {
    let pal = kmeans_palette(img, k, weights, repeat);
    match dither {
        Dither::Ordered => ordered_dither(img, &pal),
        Dither::ErrorDiffusion => error_diffusion(img, &pal),
        Dither::None => apply_palette(img, &pal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_color_on_a_black_source_is_finite() {
        let black = Img::new(3, 16, 16);
        let target = Img::from_vec(
            3,
            16,
            16,
            (0..768).map(|i| (i % 97) as f32 / 96.0).collect(),
        );
        let out = match_color(&black, &target);
        assert!(out.data.iter().all(|v| v.is_finite()));
    }
    use crate::pixeloe::fixtures::{assert_close, img};

    fn ramp(h: usize, w: usize) -> Img {
        let n = h * w;
        Img::from_vec(3, h, w, (0..3 * n).map(|i| (i % 7) as f32 / 6.0).collect())
    }

    #[test]
    fn one_colour_ordered_dither_does_not_panic() {
        let out = quantize_and_dither(&ramp(4, 4), None, 1, Dither::Ordered, false);
        assert_eq!((out.h, out.w), (4, 4));
    }

    #[test]
    fn error_diffusion_on_single_column_and_row() {
        for (h, w) in [(3, 1), (1, 3), (1, 1)] {
            let out = quantize_and_dither(&ramp(h, w), None, 2, Dither::ErrorDiffusion, false);
            assert!(out.data.iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn f16_rounding() {
        assert_eq!(round_f16(1.0), 1.0);
        assert_eq!(round_f16(1.0 + 1.0 / 4096.0), 1.0); // below half a half-ulp
        assert_eq!(round_f16(1.0 + 3.0 / 2048.0), 1.0 + 2.0 / 1024.0); // tie -> even
        assert_eq!(round_f16(65504.0), 65504.0);
        assert_eq!(round_f16(1e-8), 0.0);
        assert_eq!(round_f16(6.0e-8), 2f32.powi(-24)); // subnormal
    }

    #[test]
    fn gaussian_kernels_match() {
        for r in [2, 4] {
            assert_close(&format!("gauss_kernel_{r}"), &gaussian_kernel(r), 0.0);
        }
    }

    #[test]
    fn wavelet_and_match_color_match() {
        let (expanded, _) = crate::pixeloe::outline::outline_expansion(
            &img("a_img"),
            2,
            2,
            4,
            crate::pixeloe::outline::Mapping::Current,
            crate::pixeloe::outline::Normalize::Global,
        );
        for r in [2, 4] {
            assert_close(
                &format!("a_wavelet_blur_{r}"),
                &wavelet_blur(&expanded, r),
                1e-5,
            );
        }
        assert_close(
            "a_match_color",
            &match_color(&expanded, &img("a_img")),
            1e-5,
        );
    }

    #[test]
    fn kmeans_and_dither_match() {
        let down = crate::pixeloe::downscale::contrast(&img("a_match_color"), 4);
        let pal = kmeans_palette(&down, 16, None, false);
        let pal_img = Img::from_vec(1, 16, 3, pal.iter().flatten().cloned().collect());
        assert_close("a_kmeans_pal", &pal_img, 1e-5);
        assert_close("a_kmeans_q", &apply_palette(&down, &pal), 1e-5);
        assert_close(
            "a_kmeans5_q",
            &apply_palette(&down, &kmeans_palette(&down, 5, None, false)),
            1e-5,
        );
        for (name, d) in [
            ("ordered", Dither::Ordered),
            ("error_diffusion", Dither::ErrorDiffusion),
            ("none", Dither::None),
        ] {
            assert_close(
                &format!("a_dither_{name}"),
                &quantize_and_dither(&down, None, 16, d, false),
                1e-5,
            );
        }
    }
}
