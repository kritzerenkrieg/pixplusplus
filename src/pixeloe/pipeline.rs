//! PixelOE `pixelize` (`pixeloe/torch/pixelize.py`), torch backend.

use super::color::{match_color, quantize_and_dither, Dither};
use super::downscale::{contrast, interpolate, k_centroid, lanczos, Interp};
use super::outline::{expansion_weight, outline_expansion, Mapping, Normalize};
use super::sharpen::{laplacian, unsharp};
use super::Img;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Contrast,
    KCentroid,
    Lanczos,
    Interp(Interp),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sharpen {
    Unsharp,
    Laplacian,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quant {
    Kmeans,
    WeightedKmeans,
    RepeatKmeans,
}

/// `pixelize` arguments; [`Default`] gives upstream's defaults.
#[derive(Clone, Debug)]
pub struct Options {
    pub pixel_size: usize,
    pub thickness: usize,
    pub mode: Mode,
    pub sharpen: Option<Sharpen>,
    pub sharpen_factor: f32,
    pub color_match: bool,
    /// `do_quant` with `num_colors` colours.
    pub quant: Option<usize>,
    pub quant_mode: Quant,
    pub dither: Dither,
    /// Upstream's `match_color(quantised, down)` after quantising. It brings back continuous
    /// tones, so the output has more than `quant` colours; the CLI turns it off (G10).
    pub post_match: bool,
    pub post_upscale: bool,
    pub mapping: Mapping,
    pub normalize: Normalize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            pixel_size: 6,
            thickness: 3,
            mode: Mode::Contrast,
            sharpen: None,
            sharpen_factor: 0.5,
            color_match: true,
            quant: None,
            quant_mode: Quant::Kmeans,
            dither: Dither::Ordered,
            post_match: true,
            post_upscale: true,
            mapping: Mapping::Current,
            normalize: Normalize::Global,
        }
    }
}

/// Replicate padding, `F.pad(..., mode="replicate")`.
fn pad_replicate(img: &Img, top: usize, bottom: usize, left: usize, right: usize) -> Img {
    let (h, w) = (img.h + top + bottom, img.w + left + right);
    let mut out = Img::new(img.c, h, w);
    for c in 0..img.c {
        for y in 0..h {
            let sy = (y as isize - top as isize).clamp(0, img.h as isize - 1) as usize;
            for x in 0..w {
                let sx = (x as isize - left as isize).clamp(0, img.w as isize - 1) as usize;
                out.set(c, y, x, img.at(c, sy, sx));
            }
        }
    }
    out
}

/// PixelOE `pixelize(img, ...)` on a 0..1 RGB image.
pub fn pixelize(img: &Img, o: &Options) -> Img {
    let ps = o.pixel_size;
    let (mut out_h, mut out_w) = (img.h / ps, img.w / ps);
    let pad_h = ps
        - if img.h.is_multiple_of(ps) {
            ps
        } else {
            img.h % ps
        };
    let pad_w = ps
        - if img.w.is_multiple_of(ps) {
            ps
        } else {
            img.w % ps
        };
    let padded;
    let img = if pad_h > 0 || pad_w > 0 {
        padded = pad_replicate(
            img,
            pad_h / 2,
            pad_h - pad_h / 2,
            pad_w / 2,
            pad_w - pad_w / 2,
        );
        out_h += 1;
        out_w += 1;
        &padded
    } else {
        img
    };
    let target_size = ((out_h * out_w) as f64).sqrt();

    let (mut expanded, oe_weights) = if o.thickness > 0 {
        let (e, w) = outline_expansion(img, o.thickness, o.thickness, ps, o.mapping, o.normalize);
        (e, Some(w))
    } else {
        (img.clone(), None)
    };
    match o.sharpen {
        Some(Sharpen::Unsharp) => expanded = unsharp(&expanded, o.sharpen_factor),
        Some(Sharpen::Laplacian) => expanded = laplacian(&expanded, o.sharpen_factor),
        None => {}
    }

    let weighted = o.quant.is_some() && o.quant_mode != Quant::Kmeans;
    let weights = weighted.then(|| {
        let w = oe_weights
            .unwrap_or_else(|| expansion_weight(img, ps, ps / 2, o.mapping, o.normalize))
            .map(|v| (v * 2.0 - 1.0).abs() * v);
        let gamma = (target_size / 512.0) as f32;
        interpolate(&w, out_h, out_w, Interp::Bilinear)
            .map(|v| v.powf(gamma))
            .data
    });

    if o.color_match {
        expanded = match_color(&expanded, img);
    }

    let down = match o.mode {
        Mode::Contrast => contrast(&expanded, ps),
        Mode::KCentroid => k_centroid(&expanded, ps, 2),
        Mode::Lanczos => lanczos(&expanded, out_h, out_w),
        Mode::Interp(m) => interpolate(&expanded, out_h, out_w, m),
    };

    let fin = match o.quant {
        Some(k) => {
            let q = quantize_and_dither(
                &down,
                weights.as_deref(),
                k,
                o.dither,
                o.quant_mode == Quant::RepeatKmeans,
            );
            if o.post_match {
                match_color(&q, &down)
            } else {
                q
            }
        }
        None => down,
    };

    if o.post_upscale {
        interpolate(&fin, fin.h * ps, fin.w * ps, Interp::NearestExact)
    } else {
        fin
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixeloe::fixtures::{assert_close, img};

    fn base() -> Options {
        Options {
            pixel_size: 4,
            post_upscale: false,
            ..Options::default()
        }
    }

    #[test]
    fn pixelize_configs_match() {
        let a = img("a_img");
        let q = |mode, dither| Options {
            quant: Some(16),
            quant_mode: mode,
            dither,
            ..base()
        };
        let cfgs: Vec<(&str, Options)> = vec![
            ("default", base()),
            (
                "harness",
                Options {
                    thickness: 2,
                    ..base()
                },
            ),
            (
                "t0",
                Options {
                    thickness: 0,
                    ..base()
                },
            ),
            (
                "nomatch",
                Options {
                    color_match: false,
                    ..base()
                },
            ),
            (
                "lanczos",
                Options {
                    mode: Mode::Lanczos,
                    ..base()
                },
            ),
            (
                "bilinear",
                Options {
                    mode: Mode::Interp(Interp::Bilinear),
                    ..base()
                },
            ),
            (
                "unsharp",
                Options {
                    sharpen: Some(Sharpen::Unsharp),
                    ..base()
                },
            ),
            (
                "laplacian",
                Options {
                    sharpen: Some(Sharpen::Laplacian),
                    ..base()
                },
            ),
            ("quant_ordered", q(Quant::Kmeans, Dither::Ordered)),
            ("quant_ed", q(Quant::Kmeans, Dither::ErrorDiffusion)),
            ("quant_none", q(Quant::Kmeans, Dither::None)),
            ("quant_weighted", q(Quant::WeightedKmeans, Dither::None)),
            ("quant_repeat", q(Quant::RepeatKmeans, Dither::None)),
            (
                "map_ratio",
                Options {
                    mapping: Mapping::ContrastRatio,
                    ..base()
                },
            ),
            (
                "map_gated",
                Options {
                    mapping: Mapping::ContrastGated,
                    ..base()
                },
            ),
        ];
        let mut fails = vec![];
        for (name, o) in &cfgs {
            let fx = format!("a_pixelize_{name}");
            let got = pixelize(&a, o);
            let d = crate::pixeloe::fixtures::max_diff(&got, &img(&fx));
            eprintln!("{fx}: max |diff| {d:.2e}");
            if d > 1e-5 {
                fails.push(fx);
            }
        }
        assert!(fails.is_empty(), "over 1e-5: {fails:?}");
    }

    /// G10: without the post-quantise match the output uses at most `quant` colours.
    #[test]
    fn quant_without_post_match_is_hard_limit() {
        let o = Options {
            quant: Some(5),
            post_match: false,
            ..base()
        };
        let out = pixelize(&img("a_img"), &o);
        let n = out.h * out.w;
        let mut cols: Vec<[u32; 3]> = (0..n)
            .map(|i| [0, 1, 2].map(|c| out.data[c * n + i].to_bits()))
            .collect();
        cols.sort_unstable();
        cols.dedup();
        assert!(cols.len() <= 5, "{} colours", cols.len());
    }

    #[test]
    fn padded_input_matches() {
        let b = img("b_img");
        assert_close("b_pixelize_default", &pixelize(&b, &base()), 1e-5);
        let up = Options {
            thickness: 2,
            post_upscale: true,
            ..base()
        };
        assert_close("b_pixelize_upscaled", &pixelize(&b, &up), 1e-5);
    }
}
