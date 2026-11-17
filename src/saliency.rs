//! Automatic importance map: spectral-residual saliency (Hou & Zhang 2007, "Saliency Detection:
//! A Spectral Residual Approach", CVPR).
//!
//! Reproduces the Python `spectral_residual` that produced the showcase and benchmark maps
//! (E7), step for step, including Pillow 13's fixed-point rounding (`libImaging/Resample.c`,
//! `BoxBlur.c`, `Convert.c`), so `--importance auto` gives the same map as the harness:
//!
//! 1. RGB -> L (ITU-R 601-2, Pillow `rgb2l`).
//! 2. BOX resize to 64 wide (height `round(h * 64 / w)`, at least 8).
//! 3. log-amplitude spectrum minus its 3x3 wrap-around mean, keep the phase, inverse FFT, |.|^2.
//! 4. Scale to 0..255 as float32, truncate to L (Pillow `f2l`).
//! 5. BILINEAR resize back to the input size.
//! 6. Gaussian blur, radius = width / 40 (Pillow's 3-pass extended box blur).
//! 7. Stretch to the full range: `(a - min) / (ptp + 1e-9) * 255`, truncated.

use std::f64::consts::PI;

/// Width of the spectral-residual working image (Hou & Zhang use 64).
const SR_WIDTH: usize = 64;
/// Minimum working height.
const SR_MIN_HEIGHT: usize = 8;
/// Gaussian blur radius as a fraction of the image width (harness: `w / 40`).
const BLUR_DIV: f64 = 40.0;
/// Pillow's GaussianBlur default number of box passes.
const BLUR_PASSES: usize = 3;
/// Pillow `Resample.c` fixed-point precision for 8-bit images.
const PRECISION_BITS: u32 = 32 - 8 - 2;

/// Saliency map, one byte per pixel (row-major, `w * h`), white = salient.
pub fn spectral_residual(rgb: &[[u8; 3]], w: usize, h: usize) -> Vec<u8> {
    assert_eq!(rgb.len(), w * h);
    let gray: Vec<u8> = rgb
        .iter()
        .map(|p| {
            ((p[0] as u32 * 19595 + p[1] as u32 * 38470 + p[2] as u32 * 7471 + 0x8000) >> 16) as u8
        })
        .collect();

    let sw = SR_WIDTH;
    let sh = ((h as f64 * sw as f64 / w as f64).round_ties_even() as usize).max(SR_MIN_HEIGHT);
    let small = resample(&gray, w, h, sw, sh, Filter::Box);

    let sal = residual(&small, sw, sh);
    let max = sal.iter().cloned().fold(0.0f64, f64::max);
    let sal8: Vec<u8> = sal
        .iter()
        .map(|&v| {
            let v = if max > 0.0 {
                (v / max * 255.0) as f32
            } else {
                0.0
            };
            if v <= 0.0 {
                0
            } else if v >= 255.0 {
                255
            } else {
                v as u8
            }
        })
        .collect();

    let big = resample(&sal8, sw, sh, w, h, Filter::Bilinear);
    let blurred = gaussian_blur(&big, w, h, (w as f64 / BLUR_DIV) as f32);

    let lo = *blurred.iter().min().unwrap() as f64;
    let hi = *blurred.iter().max().unwrap() as f64;
    let ptp = hi - lo;
    blurred
        .iter()
        .map(|&v| ((v as f64 - lo) / (ptp + 1e-9) * 255.0) as u8)
        .collect()
}

/// Spectral residual of a small grayscale image (values / 255), returns |ifft|^2.
fn residual(img: &[u8], w: usize, h: usize) -> Vec<f64> {
    let x: Vec<(f64, f64)> = img.iter().map(|&v| (v as f64 / 255.0, 0.0)).collect();
    let f = dft2(&x, w, h, false);
    let log_a: Vec<f64> = f
        .iter()
        .map(|&(re, im)| (re.hypot(im) + 1e-9).ln())
        .collect();
    let mut spec = vec![(0.0, 0.0); w * h];
    for y in 0..h {
        for xx in 0..w {
            let mut avg = 0.0;
            for dy in 0..3 {
                for dx in 0..3 {
                    let yy = (y + h + dy - 1) % h;
                    let xs = (xx + w + dx - 1) % w;
                    avg += log_a[yy * w + xs];
                }
            }
            avg /= 9.0;
            let i = y * w + xx;
            let mag = (log_a[i] - avg).exp();
            let ang = f[i].1.atan2(f[i].0);
            spec[i] = (mag * ang.cos(), mag * ang.sin());
        }
    }
    dft2(&spec, w, h, true)
        .iter()
        .map(|&(re, im)| re * re + im * im)
        .collect()
}

/// Direct 2-D DFT (rows then columns). The working image is tiny (64 x ~48), so O(n^2) is fine.
/// `inverse` applies the 1/(w*h) normalisation, like `numpy.fft.ifft2`.
fn dft2(x: &[(f64, f64)], w: usize, h: usize, inverse: bool) -> Vec<(f64, f64)> {
    let sign = if inverse { 1.0 } else { -1.0 };
    let dft1 = |line: &[(f64, f64)]| -> Vec<(f64, f64)> {
        let n = line.len();
        (0..n)
            .map(|k| {
                let mut acc = (0.0, 0.0);
                for (t, &(re, im)) in line.iter().enumerate() {
                    let a = sign * 2.0 * PI * ((k * t) % n) as f64 / n as f64;
                    let (s, c) = a.sin_cos();
                    acc.0 += re * c - im * s;
                    acc.1 += re * s + im * c;
                }
                acc
            })
            .collect()
    };
    let mut rows = Vec::with_capacity(w * h);
    for y in 0..h {
        rows.extend(dft1(&x[y * w..(y + 1) * w]));
    }
    let mut out = vec![(0.0, 0.0); w * h];
    for xx in 0..w {
        let col: Vec<_> = (0..h).map(|y| rows[y * w + xx]).collect();
        for (y, v) in dft1(&col).into_iter().enumerate() {
            out[y * w + xx] = v;
        }
    }
    if inverse {
        let s = 1.0 / (w * h) as f64;
        for v in &mut out {
            v.0 *= s;
            v.1 *= s;
        }
    }
    out
}

#[derive(Clone, Copy)]
enum Filter {
    Box,
    Bilinear,
}

impl Filter {
    fn support(self) -> f64 {
        match self {
            Filter::Box => 0.5,
            Filter::Bilinear => 1.0,
        }
    }
    fn eval(self, x: f64) -> f64 {
        match self {
            Filter::Box => {
                if x > -0.5 && x <= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            Filter::Bilinear => {
                let x = x.abs();
                if x < 1.0 {
                    1.0 - x
                } else {
                    0.0
                }
            }
        }
    }
}

/// Pillow `precompute_coeffs` + `normalize_coeffs_8bpc`: per output index, (first input index,
/// fixed-point weights).
fn coeffs(in_size: usize, out_size: usize, f: Filter) -> Vec<(usize, Vec<i32>)> {
    let scale = in_size as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = f.support() * filterscale;
    let inv = 1.0 / filterscale;
    (0..out_size)
        .map(|xx| {
            let center = (xx as f64 + 0.5) * scale;
            let xmin = ((center - support + 0.5) as i64).max(0) as usize;
            let xmax = ((center + support + 0.5) as i64).min(in_size as i64) as usize;
            let mut k: Vec<f64> = (xmin..xmax)
                .map(|x| f.eval((x as f64 - center + 0.5) * inv))
                .collect();
            let ww: f64 = k.iter().sum();
            if ww != 0.0 {
                k.iter_mut().for_each(|v| *v /= ww);
            }
            let fixed = k
                .iter()
                .map(|&v| {
                    let s = v * (1u32 << PRECISION_BITS) as f64;
                    if v < 0.0 {
                        (-0.5 + s) as i32
                    } else {
                        (0.5 + s) as i32
                    }
                })
                .collect();
            (xmin, fixed)
        })
        .collect()
}

fn clip8(ss: i32) -> u8 {
    (ss >> PRECISION_BITS).clamp(0, 255) as u8
}

fn pass_h(src: &[u8], w: usize, h: usize, nw: usize, f: Filter) -> Vec<u8> {
    let c = coeffs(w, nw, f);
    let mut out = vec![0u8; nw * h];
    for y in 0..h {
        let line = &src[y * w..(y + 1) * w];
        for (xx, (xmin, k)) in c.iter().enumerate() {
            let mut ss: i32 = 1 << (PRECISION_BITS - 1);
            for (j, &kv) in k.iter().enumerate() {
                ss += line[xmin + j] as i32 * kv;
            }
            out[y * nw + xx] = clip8(ss);
        }
    }
    out
}

fn pass_v(src: &[u8], w: usize, h: usize, nh: usize, f: Filter) -> Vec<u8> {
    let c = coeffs(h, nh, f);
    let mut out = vec![0u8; w * nh];
    for (yy, (ymin, k)) in c.iter().enumerate() {
        for x in 0..w {
            let mut ss: i32 = 1 << (PRECISION_BITS - 1);
            for (j, &kv) in k.iter().enumerate() {
                ss += src[(ymin + j) * w + x] as i32 * kv;
            }
            out[yy * w + x] = clip8(ss);
        }
    }
    out
}

/// Pillow `ImagingResampleInner` for an 8-bit single-band image, including its pass order.
fn resample(src: &[u8], w: usize, h: usize, nw: usize, nh: usize, f: Filter) -> Vec<u8> {
    let need_h = nw != w;
    let need_v = nh != h;
    let horizontal_first = !(h > nh && (h - nh) as i64 > (w as i64 - nw as i64) * 2);
    let mut cur = src.to_vec();
    let mut cw = w;
    if horizontal_first {
        if need_h {
            cur = pass_h(&cur, cw, h, nw, f);
            cw = nw;
        }
        if need_v {
            cur = pass_v(&cur, cw, h, nh, f);
        }
    } else {
        if need_v {
            cur = pass_v(&cur, cw, h, nh, f);
        }
        if need_h {
            cur = pass_h(&cur, cw, nh, nw, f);
        }
    }
    cur
}

/// Pillow `_gaussian_blur_radius` (float32 arithmetic, as in C).
fn box_radius(radius: f32, passes: usize) -> f32 {
    let sigma2 = radius * radius / passes as f32;
    let big_l = (12.0 * sigma2 as f64 + 1.0).sqrt() as f32;
    let l = ((big_l as f64 - 1.0) / 2.0).floor() as f32;
    let mut a = (2.0 * l + 1.0) * (l * (l + 1.0) - 3.0 * sigma2);
    a /= 6.0 * (sigma2 - (l + 1.0) * (l + 1.0));
    l + a
}

/// Pillow `ImagingLineBoxBlur8` on one line.
fn box_line(line: &[u8], out: &mut [u8], radius: usize, ww: u32, fw: u32) {
    let n = line.len();
    let lastx = n - 1;
    let edge_a = (radius + 1).min(n);
    let edge_b = n.saturating_sub(radius + 1);
    let px = |i: usize| line[i] as u32;
    let mut acc: u32 = px(0) * (radius as u32 + 1);
    for x in 0..edge_a.saturating_sub(1) {
        acc = acc.wrapping_add(px(x));
    }
    acc = acc.wrapping_add(px(lastx).wrapping_mul((radius as u32 + 1).wrapping_sub(edge_a as u32)));
    let mut step = |x: usize, sub: usize, add: usize, far_l: usize, far_r: usize, acc: &mut u32| {
        *acc = acc.wrapping_add(px(add)).wrapping_sub(px(sub));
        let bulk = acc
            .wrapping_mul(ww)
            .wrapping_add((px(far_l) + px(far_r)).wrapping_mul(fw));
        out[x] = (bulk.wrapping_add(1 << 23) >> 24) as u8;
    };
    if edge_a <= edge_b {
        for x in 0..edge_a {
            step(x, 0, x + radius, 0, x + radius + 1, &mut acc);
        }
        for x in edge_a..edge_b {
            step(
                x,
                x - radius - 1,
                x + radius,
                x - radius - 1,
                x + radius + 1,
                &mut acc,
            );
        }
        for x in edge_b..=lastx {
            step(x, x - radius - 1, lastx, x - radius - 1, lastx, &mut acc);
        }
    } else {
        for x in 0..edge_b {
            step(x, 0, x + radius, 0, x + radius + 1, &mut acc);
        }
        for x in edge_b..edge_a {
            step(x, 0, lastx, 0, lastx, &mut acc);
        }
        for x in edge_a..=lastx {
            step(x, x - radius - 1, lastx, x - radius - 1, lastx, &mut acc);
        }
    }
}

/// Pillow `ImagingHorizontalBoxBlur`, `passes` times over every row.
fn box_rows(img: &mut [u8], w: usize, h: usize, fr: f32, passes: usize) {
    let radius = fr as usize;
    let ww = (16_777_216.0f32 / (fr * 2.0 + 1.0)) as u32;
    let fw = (16_777_216u32.wrapping_sub((radius as u32 * 2 + 1).wrapping_mul(ww))) / 2;
    let mut tmp = vec![0u8; w];
    for _ in 0..passes {
        for y in 0..h {
            let row = &mut img[y * w..(y + 1) * w];
            box_line(row, &mut tmp, radius, ww, fw);
            row.copy_from_slice(&tmp);
        }
    }
}

fn transpose(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            out[x * h + y] = src[y * w + x];
        }
    }
    out
}

/// Pillow `ImageFilter.GaussianBlur(radius)` on an 8-bit single-band image.
fn gaussian_blur(src: &[u8], w: usize, h: usize, radius: f32) -> Vec<u8> {
    if radius == 0.0 {
        return src.to_vec();
    }
    let fr = box_radius(radius, BLUR_PASSES);
    let mut img = src.to_vec();
    if fr != 0.0 {
        box_rows(&mut img, w, h, fr, BLUR_PASSES);
        let mut t = transpose(&img, w, h);
        box_rows(&mut t, h, w, fr, BLUR_PASSES);
        img = transpose(&t, h, w);
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference produced by `bench/saliency_ref.py` (the harness's `spectral_residual`).
    #[test]
    fn matches_python_spectral_residual() {
        for name in ["saliency", "saliency_tall"] {
            check_fixture(name);
        }
    }

    fn check_fixture(name: &str) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
        let src = image::open(format!("{dir}/{name}_src.png"))
            .unwrap()
            .to_rgb8();
        let want = image::open(format!("{dir}/{name}_ref.png"))
            .unwrap()
            .to_luma8();
        let (w, h) = src.dimensions();
        let rgb: Vec<[u8; 3]> = src.pixels().map(|p| p.0).collect();
        let got = spectral_residual(&rgb, w as usize, h as usize);
        let diff: Vec<i32> = got
            .iter()
            .zip(want.as_raw())
            .map(|(&a, &b)| (a as i32 - b as i32).abs())
            .collect();
        let mean = diff.iter().sum::<i32>() as f64 / diff.len() as f64;
        let max = *diff.iter().max().unwrap();
        eprintln!("{name} vs python: mean |diff| {mean:.4} levels, max {max}");
        assert!(mean <= 1.0, "{name}: mean |diff| {mean} > 1/255");
    }
}
