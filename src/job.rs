//! Image-in, image-out glue shared by the CLI and the GUI (J2): output size, input resize,
//! importance map, and both methods (E8) from an `RgbImage` to an `RgbImage`.

use crate::pixeloe::pipeline::{pixelize, Options};
use crate::pixeloe::Img;
use crate::{run, spectral_residual, ColorAdjust, Input, Output, Params, F};
use image::{imageops::FilterType, GrayImage, RgbImage};
use std::time::Instant;

/// Output grid for an `iw`×`ih` input: `width`×`height` if both are given, else `size` on the
/// long side with the aspect kept.
pub fn output_size(
    iw: u32,
    ih: u32,
    size: usize,
    width: Option<usize>,
    height: Option<usize>,
) -> (usize, usize) {
    if let (Some(w), Some(h)) = (width, height) {
        return (w, h);
    }
    let s = size as f64;
    if iw >= ih {
        (size, ((ih as f64 * s / iw as f64).round() as usize).max(1))
    } else {
        (((iw as f64 * s / ih as f64).round() as usize).max(1), size)
    }
}

/// I1: PIA needs at least one input pixel per output pixel. An output grid larger than the
/// input is scaled down (aspect kept) to fit inside it.
pub fn cap_output(ow: usize, oh: usize, w: usize, h: usize) -> (usize, usize) {
    if ow <= w && oh <= h {
        return (ow, oh);
    }
    let f = (w as f64 / ow as f64).min(h as f64 / oh as f64);
    (
        ((ow as f64 * f).round() as usize).clamp(1, w),
        ((oh as f64 * f).round() as usize).clamp(1, h),
    )
}

/// PIA importance map source (C1, E7).
pub enum Importance {
    None,
    /// Spectral-residual saliency of the (resized) input (E7).
    Auto,
    /// Grayscale map, white = more palette mass; resized to the input.
    Map(GrayImage),
}

pub struct PiaResult {
    pub image: RgbImage,
    pub output: Output,
    /// The parameters that ran, with the output grid after the I1 cap.
    pub params: Params,
    /// Input size after the `input_factor` downscale.
    pub input_w: u32,
    pub input_h: u32,
    /// The I1 cap warning, if the grid was capped.
    pub warning: Option<String>,
}

/// PIA on `img`. `params.out_w`/`out_h` is the requested grid; the input is downscaled to at
/// most `input_factor` × that grid (B3, 0 = off), then the grid is capped at the input (I1).
/// `adjust.pre` runs on the resized input, `adjust.post` on the output palette (K4); the
/// solver's own `params.saturation` (B5) is separate and comes first.
pub fn pia_image(
    mut img: RgbImage,
    mut params: Params,
    input_factor: f64,
    importance: &Importance,
    adjust: &ColorAdjust,
) -> PiaResult {
    let (iw, ih) = img.dimensions();
    let (ow, oh) = (params.out_w, params.out_h);
    if input_factor > 0.0 {
        let (mw, mh) = (
            ((ow as f64 * input_factor).round() as u32).min(iw),
            ((oh as f64 * input_factor).round() as u32).min(ih),
        );
        if iw > mw || ih > mh {
            img = image::imageops::resize(&img, mw, mh, FilterType::Lanczos3);
        }
    }
    adjust.pre.apply(&mut img);
    let (w, h) = img.dimensions();
    let (cw, ch) = cap_output(ow, oh, w as usize, h as usize);
    let warning = ((cw, ch) != (ow, oh))
        .then(|| format!("output {ow}x{oh} exceeds the {w}x{h} input; capped to {cw}x{ch} (I1)"));
    params.out_w = cw;
    params.out_h = ch;

    let rgb: Vec<[F; 3]> = img.pixels().map(|p| p.0.map(|v| v as F / 255.0)).collect();
    let imp: Option<Vec<F>> = match importance {
        Importance::None => None,
        Importance::Auto => {
            let rgb8: Vec<[u8; 3]> = img.pixels().map(|p| p.0).collect();
            let map = spectral_residual(&rgb8, w as usize, h as usize);
            Some(map.iter().map(|&v| v as F / 255.0).collect())
        }
        Importance::Map(g) => {
            let resized;
            let g = if g.dimensions() != (w, h) {
                resized = image::imageops::resize(g, w, h, FilterType::Triangle);
                &resized
            } else {
                g
            };
            Some(g.pixels().map(|p| p.0[0] as F / 255.0).collect())
        }
    };
    let input = Input {
        width: w as usize,
        height: h as usize,
        rgb: &rgb,
        importance: imp.as_deref(),
    };
    let mut output = run(&input, &params);
    let mut image =
        RgbImage::from_raw(output.width as u32, output.height as u32, output.rgb8()).unwrap();
    let map = adjust.post.colour_map(&image);
    if !map.is_empty() {
        for c in output.palette_srgb.iter_mut() {
            if let Some(&m) = map.get(c) {
                *c = m;
            }
        }
        image =
            RgbImage::from_raw(output.width as u32, output.height as u32, output.rgb8()).unwrap();
    }
    PiaResult {
        image,
        output,
        params,
        input_w: w,
        input_h: h,
        warning,
    }
}

pub struct PixeloeResult {
    pub image: RgbImage,
    /// Distinct output colours, sorted.
    pub palette: Vec<[u8; 3]>,
    /// The options that ran, with `pixel_size` set (G7).
    pub options: Options,
    /// Input size after the G7 resize.
    pub input_w: usize,
    pub input_h: usize,
    pub solve_ms: f64,
}

/// The PixelOE options the CLI runs with no `--oe-*` flags: upstream's defaults (G9), but no
/// colour match after quantising (G10) and no upscale (the output is one pixel per cell).
pub fn pixeloe_defaults() -> Options {
    Options {
        post_match: false,
        post_upscale: false,
        ..Options::default()
    }
}

/// PixelOE on `img` for an `ow`×`oh` output (G4, G7, G8). With `input_factor` > 0 the pixel size
/// is its rounded value and the input is resized to exactly pixel size × the output; with 0 the
/// pixel size is round(long side / `size`) on the unresized input. `options.pixel_size` is
/// overwritten.
pub fn pixeloe_image(
    mut img: RgbImage,
    ow: usize,
    oh: usize,
    size: usize,
    input_factor: f64,
    mut options: Options,
    adjust: &ColorAdjust,
) -> PixeloeResult {
    let (iw, ih) = img.dimensions();
    let ps = if input_factor > 0.0 {
        let ps = (input_factor.round() as usize).max(1);
        let (tw, th) = ((ow * ps) as u32, (oh * ps) as u32);
        if (iw, ih) != (tw, th) {
            img = image::imageops::resize(&img, tw, th, FilterType::Lanczos3);
        }
        ps
    } else {
        ((iw.max(ih) as f64 / size as f64).round() as usize).max(1)
    };
    options.pixel_size = ps;
    adjust.pre.apply(&mut img);
    let (w, h) = (img.width() as usize, img.height() as usize);
    let n = w * h;
    let mut data = vec![0.0f32; 3 * n];
    for (i, p) in img.pixels().enumerate() {
        for c in 0..3 {
            data[c * n + i] = p.0[c] as f32 / 255.0;
        }
    }
    let t = Instant::now();
    let out = pixelize(&Img::from_vec(3, h, w, data), &options);
    let solve_ms = t.elapsed().as_secs_f64() * 1000.0;

    let m = out.h * out.w;
    let rgb8: Vec<u8> = (0..m)
        .flat_map(|i| (0..3).map(move |c| (c, i)))
        .map(|(c, i)| (out.data[c * m + i].clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    let mut image = RgbImage::from_raw(out.w as u32, out.h as u32, rgb8).unwrap();
    adjust.post.apply(&mut image);
    let mut palette: Vec<[u8; 3]> = image.as_raw().as_chunks::<3>().0.to_vec();
    palette.sort_unstable();
    palette.dedup();
    PixeloeResult {
        image,
        palette,
        options,
        input_w: w,
        input_h: h,
        solve_ms,
    }
}

/// Nearest-neighbour upscale by `f`, for previews.
pub fn upscale(img: &RgbImage, f: u32) -> RgbImage {
    image::imageops::resize(img, img.width() * f, img.height() * f, FilterType::Nearest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_output_fits_inside_input() {
        assert_eq!(cap_output(128, 102, 100, 80), (100, 80));
        assert_eq!(cap_output(50, 300, 100, 80), (13, 80));
        assert_eq!(cap_output(100, 80, 100, 80), (100, 80));
        assert_eq!(cap_output(16, 9, 1, 1), (1, 1));
    }

    #[test]
    fn pia_never_enlarges_either_axis() {
        // A wide, short input with a square grid: the height must not be stretched (I1).
        let img = RgbImage::from_fn(200, 5, |x, y| image::Rgb([x as u8, (y * 40) as u8, 90]));
        let r = pia_image(
            img,
            Params::new(16, 16, 4),
            4.0,
            &Importance::None,
            &ColorAdjust::default(),
        );
        assert!(
            r.input_w <= 200 && r.input_h <= 5,
            "{}x{}",
            r.input_w,
            r.input_h
        );
        assert!(r.params.out_h <= 5);
    }

    #[test]
    fn output_size_keeps_aspect() {
        assert_eq!(output_size(512, 320, 128, None, None), (128, 80));
        assert_eq!(output_size(320, 512, 128, None, None), (80, 128));
        assert_eq!(output_size(512, 320, 128, Some(10), Some(20)), (10, 20));
        assert_eq!(output_size(512, 320, 128, Some(10), None), (128, 80));
    }
}
