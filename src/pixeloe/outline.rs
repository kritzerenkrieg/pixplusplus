//! Contrast-aware outline expansion (`pixeloe/torch/outline.py`): a per-pixel weight decides
//! between erosion (thin dark lines grow) and dilation (thin bright lines grow), followed by a
//! small morphological close/open so the lines survive the downscale.

use super::lab::rgb_to_lab;
use super::morph::{dilate, erode, kernel, local_stat, Stat};
use super::Img;

/// PixelOE `avg_scale` default: weight of the local background brightness.
pub const AVG_SCALE: f32 = 10.0;
/// PixelOE `dist_scale` default: weight of the bright/dark detail asymmetry.
pub const DIST_SCALE: f32 = 3.0;

/// Upstream `weight_mapping` options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapping {
    /// "current" (alias "polarity"), the default.
    Current,
    ContrastRatio,
    ContrastGated,
}

/// Upstream `weight_normalize` options ("per_image" equals "global" for a single image).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Normalize {
    None,
    Global,
}

#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn normalize(mut w: Img, mode: Normalize) -> Img {
    if mode == Normalize::Global {
        let lo = w.data.iter().cloned().fold(f32::INFINITY, f32::min);
        let hi = w.data.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let d = hi - lo + 1e-8;
        w.data.iter_mut().for_each(|v| *v = (*v - lo) / d);
    }
    w
}

/// Mean in f64 (torch's f32 reduction is pairwise; f64 is at least as accurate).
fn mean(v: &[f32]) -> f32 {
    (v.iter().map(|&x| x as f64).sum::<f64>() / v.len() as f64) as f32
}

/// PixelOE `expansion_weight(img, k, stride, ...)`: 1 = erode (dark detail), 0 = dilate.
pub fn expansion_weight(
    img: &Img,
    k: usize,
    stride: usize,
    mapping: Mapping,
    norm: Normalize,
) -> Img {
    let lum = rgb_to_lab(img).channel(0).map(|v| v / 100.0);
    let med = local_stat(&lum, k * 2, stride, Stat::Median);
    let min = local_stat(&lum, k, stride, Stat::Min);
    let max = local_stat(&lum, k, stride, Stat::Max);
    let n = lum.data.len();
    let bright: Vec<f32> = (0..n).map(|i| max.data[i] - med.data[i]).collect();
    let dark: Vec<f32> = (0..n).map(|i| med.data[i] - min.data[i]).collect();
    let polarity = |i: usize| (med.data[i] - 0.5) * AVG_SCALE + (dark[i] - bright[i]) * DIST_SCALE;
    let w: Vec<f32> = match mapping {
        Mapping::Current => (0..n).map(|i| sigmoid(polarity(i))).collect(),
        Mapping::ContrastRatio => (0..n)
            .map(|i| {
                let contrast = bright[i] + dark[i];
                let asym = (bright[i] - dark[i]) / (contrast + 1e-8);
                sigmoid((med.data[i] - 0.5) * AVG_SCALE - asym * DIST_SCALE)
            })
            .collect(),
        Mapping::ContrastGated => {
            let contrast: Vec<f32> = (0..n).map(|i| bright[i] + dark[i]).collect();
            let m = mean(&contrast);
            (0..n)
                .map(|i| {
                    let gate = sigmoid((contrast[i] - m) * DIST_SCALE);
                    0.5 + (sigmoid(polarity(i)) - 0.5) * gate
                })
                .collect()
        }
    };
    normalize(Img::from_vec(1, img.h, img.w, w), norm)
}

/// PixelOE `outline_expansion(img, erode_iters, dilate_iters, k)`: returns the expanded image
/// and the weight map.
pub fn outline_expansion(
    img: &Img,
    erode_iters: usize,
    dilate_iters: usize,
    k: usize,
    mapping: Mapping,
    norm: Normalize,
) -> (Img, Img) {
    let w = expansion_weight(img, k, (k / 2).max(1), mapping, norm);
    let e = erode(img, &kernel(erode_iters), 1);
    let d = dilate(img, &kernel(dilate_iters), 1);
    let plane = img.h * img.w;
    let mut out = Img::new(img.c, img.h, img.w);
    for c in 0..img.c {
        for i in 0..plane {
            let o = c * plane + i;
            out.data[o] = e.data[o] * w.data[i] + d.data[o] * (1.0 - w.data[i]);
        }
    }
    let oc = kernel((erode_iters.max(dilate_iters).saturating_sub(1)).max(1));
    let out = erode(&out, &oc, 1);
    let out = dilate(&out, &oc, 2);
    let out = erode(&out, &oc, 1);
    (out, w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixeloe::fixtures::{assert_close, img};

    #[test]
    fn weights_match() {
        let a = img("a_img");
        for (name, m) in [
            ("current", Mapping::Current),
            ("contrast_ratio", Mapping::ContrastRatio),
            ("contrast_gated", Mapping::ContrastGated),
        ] {
            assert_close(
                &format!("a_weight_{name}"),
                &expansion_weight(&a, 4, 2, m, Normalize::Global),
                1e-5,
            );
        }
        assert_close(
            "a_weight_none",
            &expansion_weight(&a, 4, 2, Mapping::Current, Normalize::None),
            1e-5,
        );
    }

    #[test]
    fn expansion_matches() {
        let a = img("a_img");
        for t in 1..=3 {
            let (out, w) = outline_expansion(&a, t, t, 4, Mapping::Current, Normalize::Global);
            assert_close(&format!("a_expand_{t}"), &out, 1e-5);
            assert_close(&format!("a_expand_{t}_w"), &w, 1e-5);
        }
    }
}
