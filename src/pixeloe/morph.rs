//! PixelOE's soft morphology and local statistics (`pixeloe/torch/minmax.py`,
//! `outline.py::local_stat`), computed with direct loops instead of `F.unfold`, which is
//! where most of upstream's memory goes.

use super::Img;

/// PixelOE `circle_kernel(r)`: a disc of radius `r` with fractional (anti-aliased) edges,
/// computed in f64 like numpy, stored as f32.
pub fn circle_kernel(r: f64) -> Img {
    let ri = r as usize;
    let n = 2 * ri + 1;
    let offs = [
        (-0.5, -0.5),
        (-0.5, 0.5),
        (0.5, -0.5),
        (0.5, 0.5),
        (0.0, 0.5),
        (0.0, -0.5),
        (0.5, 0.0),
        (-0.5, 0.0),
    ];
    let mut k = Img::new(1, n, n);
    for i in 0..n {
        for j in 0..n {
            let d: Vec<f64> = offs
                .iter()
                .map(|(di, dj)| {
                    let a = i as f64 + di - ri as f64;
                    let b = j as f64 + dj - ri as f64;
                    (a * a + b * b).sqrt()
                })
                .collect();
            let max = d.iter().cloned().fold(f64::MIN, f64::max);
            let min = d.iter().cloned().fold(f64::MAX, f64::min);
            let v = if max <= r {
                1.0
            } else if min <= r {
                (r - min) / (max - min)
            } else {
                0.0
            };
            k.set(0, i, j, v as f32);
        }
    }
    k
}

fn crop(k: &Img, y0: usize, x0: usize, n: usize) -> Img {
    let mut out = Img::new(1, n, n);
    for y in 0..n {
        for x in 0..n {
            out.set(0, y, x, k.at(0, y0 + y, x0 + x));
        }
    }
    out
}

/// PixelOE `KERNELS[t]` for t = 1..=6.
pub fn kernel(t: usize) -> Img {
    match t {
        1 => circle_kernel(1.0),
        2 => circle_kernel(1.5),
        3 => crop(&circle_kernel(2.0), 1, 1, 3),
        4 => circle_kernel(2.5),
        5 => crop(&circle_kernel(3.0), 1, 1, 5),
        6 => circle_kernel(3.5),
        _ => panic!("PixelOE kernels exist for 1..=6, got {t}"),
    }
}

/// One pass of soft erosion (`dilate = false`) or dilation, zero-padded like `F.unfold`.
fn morph_pass(img: &Img, k: &Img, dilate: bool) -> Img {
    let (kh, kw) = (k.h, k.w);
    let (ph, pw) = (kh / 2, kw / 2);
    let mut out = Img::new(img.c, img.h, img.w);
    for c in 0..img.c {
        for y in 0..img.h {
            for x in 0..img.w {
                let mut best = if dilate {
                    f32::NEG_INFINITY
                } else {
                    f32::INFINITY
                };
                for i in 0..kh {
                    for j in 0..kw {
                        let yy = y as isize + i as isize - ph as isize;
                        let xx = x as isize + j as isize - pw as isize;
                        let v = if yy < 0 || xx < 0 || yy >= img.h as isize || xx >= img.w as isize
                        {
                            0.0
                        } else {
                            img.at(c, yy as usize, xx as usize)
                        };
                        let kv = k.at(0, i, j);
                        if dilate {
                            best = best.max(v + kv - 1.0);
                        } else {
                            best = best.min(v - kv + 1.0);
                        }
                    }
                }
                out.set(c, y, x, best);
            }
        }
    }
    out
}

/// PixelOE `erode_cont`.
pub fn erode(img: &Img, k: &Img, iterations: usize) -> Img {
    let mut x = img.clone();
    for _ in 0..iterations {
        x = morph_pass(&x, k, false);
    }
    x
}

/// PixelOE `dilate_cont` (clamped to 0..1 at the end, as upstream).
pub fn dilate(img: &Img, k: &Img, iterations: usize) -> Img {
    let mut x = img.clone();
    for _ in 0..iterations {
        x = morph_pass(&x, k, true);
    }
    x.map(|v| v.clamp(0.0, 1.0))
}

#[derive(Clone, Copy, Debug)]
pub enum Stat {
    /// torch's median: the lower middle element for an even count.
    Median,
    Min,
    Max,
}

/// PixelOE `local_stat` on a single-channel image: the statistic of each `kernel` x `kernel`
/// patch (stride `stride`, zero padding `kernel / 2`), spread back over the patch and averaged
/// where patches overlap (`F.fold(vals) / (F.fold(ones) + 1e-8)`). Accumulation follows
/// torch's col2im order, so sums round the same way.
pub fn local_stat(img: &Img, kernel: usize, stride: usize, stat: Stat) -> Img {
    assert_eq!(img.c, 1);
    let (h, w) = (img.h, img.w);
    let p = kernel / 2;
    let nh = (h + 2 * p - kernel) / stride + 1;
    let nw = (w + 2 * p - kernel) / stride + 1;
    let mut vals = vec![0.0f32; nh * nw];
    let mut buf = Vec::with_capacity(kernel * kernel);
    for py in 0..nh {
        for px in 0..nw {
            buf.clear();
            for i in 0..kernel {
                for j in 0..kernel {
                    let y = (py * stride + i) as isize - p as isize;
                    let x = (px * stride + j) as isize - p as isize;
                    buf.push(if y < 0 || x < 0 || y >= h as isize || x >= w as isize {
                        0.0
                    } else {
                        img.data[y as usize * w + x as usize]
                    });
                }
            }
            vals[py * nw + px] = match stat {
                Stat::Min => buf.iter().cloned().fold(f32::INFINITY, f32::min),
                Stat::Max => buf.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                Stat::Median => {
                    buf.sort_by(|a, b| a.total_cmp(b));
                    buf[(buf.len() - 1) / 2]
                }
            };
        }
    }
    let mut acc = vec![0.0f32; h * w];
    let mut div = vec![0.0f32; h * w];
    for i in 0..kernel {
        for j in 0..kernel {
            for py in 0..nh {
                let y = (py * stride + i) as isize - p as isize;
                if y < 0 || y >= h as isize {
                    continue;
                }
                for px in 0..nw {
                    let x = (px * stride + j) as isize - p as isize;
                    if x < 0 || x >= w as isize {
                        continue;
                    }
                    let o = y as usize * w + x as usize;
                    acc[o] += vals[py * nw + px];
                    div[o] += 1.0;
                }
            }
        }
    }
    Img::from_vec(
        1,
        h,
        w,
        acc.iter().zip(&div).map(|(a, d)| a / (d + 1e-8)).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixeloe::fixtures::{assert_close, img};
    use crate::pixeloe::lab::rgb_to_lab;

    #[test]
    fn kernels_match() {
        for t in 1..=6 {
            assert_close(&format!("kernel_{t}"), &kernel(t), 1e-7);
        }
    }

    #[test]
    fn erode_dilate_match() {
        let a = img("a_img");
        for t in 1..=3 {
            let k = kernel(t);
            assert_close(&format!("a_erode_{t}"), &erode(&a, &k, 1), 1e-6);
            assert_close(&format!("a_dilate_{t}"), &dilate(&a, &k, 1), 1e-6);
            assert_close(&format!("a_dilate_{t}_x2"), &dilate(&a, &k, 2), 1e-6);
        }
    }

    #[test]
    fn local_stats_match() {
        let lum = rgb_to_lab(&img("a_img")).channel(0).map(|v| v / 100.0);
        for (k, s) in [(8, 2), (4, 2), (5, 3)] {
            for (name, st) in [
                ("median", Stat::Median),
                ("min", Stat::Min),
                ("max", Stat::Max),
            ] {
                assert_close(
                    &format!("a_local_{name}_{k}_{s}"),
                    &local_stat(&lum, k, s, st),
                    1e-5,
                );
            }
        }
    }
}
