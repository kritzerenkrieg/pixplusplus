use clap::Parser;
use image::{imageops::FilterType, GrayImage, RgbImage};
use piaru::pixeloe::color::Dither;
use piaru::pixeloe::downscale::Interp;
use piaru::pixeloe::outline::{Mapping, Normalize};
use piaru::pixeloe::pipeline::{pixelize, Mode, Options, Quant, Sharpen};
use piaru::pixeloe::Img;
use piaru::{peak_rss_kib, run, spectral_residual, Input, Params, Space, Stats, F};
use std::path::PathBuf;
use std::time::Instant;

/// Concept art -> pixel art via Pixelated Image Abstraction (MCDA in Oklab), or PixelOE (G4).
#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    input: PathBuf,
    /// Output PNG (one pixel per superpixel).
    #[arg(short, long)]
    output: PathBuf,
    /// Output long side in pixels (aspect preserved). Ignored if --width and --height are given.
    #[arg(short, long, default_value_t = 128)]
    size: usize,
    #[arg(long)]
    width: Option<usize>,
    #[arg(long)]
    height: Option<usize>,
    /// Algorithm (E8, G4).
    #[arg(long, value_enum, default_value_t = MethodArg::Pia)]
    method: MethodArg,
    /// Palette size (hard limit). PIA default 16; PixelOE quantises only when given (G8).
    #[arg(short = 'k', long)]
    colors: Option<usize>,
    /// Downscale the input so it is at most this many times the output (0 = use input as is).
    /// Under PixelOE, also the pixel size; the input is resized to exactly this times the output
    /// (G7).
    #[arg(long, default_value_t = 4.0)]
    input_factor: f64,
    #[arg(long, value_enum, default_value_t = SpaceArg::Oklab)]
    space: SpaceArg,
    /// Grayscale importance map (white = more palette mass), resized to the input; or `auto` for
    /// spectral-residual saliency computed from the input (E7).
    #[arg(long)]
    importance: Option<PathBuf>,
    #[arg(long, default_value_t = 1.1)]
    saturation: F,
    #[arg(long, default_value_t = 35.0)]
    t0: F,
    #[arg(long, default_value_t = 45.0)]
    m: F,
    #[arg(long, default_value_t = 5000)]
    max_iters: usize,
    /// Reproduce Pyxeled's quirks (for parity checks only).
    #[arg(long)]
    pyxeled_compat: bool,
    /// Also write a nearest-neighbour upscaled preview with this factor (<output>.xN.png).
    #[arg(long)]
    preview: Option<u32>,
    /// Write run statistics (timings, iterations, peak RSS) as JSON.
    #[arg(long)]
    stats: Option<PathBuf>,
    /// Number of worker threads (default: all cores).
    #[arg(long)]
    threads: Option<usize>,

    /// PixelOE outline thickness (0 = no outline expansion). Default 3.
    #[arg(long, help_heading = "PixelOE (G9)")]
    oe_thickness: Option<usize>,
    /// PixelOE downscale mode. Default contrast.
    #[arg(long, value_enum, help_heading = "PixelOE (G9)")]
    oe_mode: Option<OeMode>,
    /// PixelOE sharpening before downscale. Default none.
    #[arg(long, value_enum, help_heading = "PixelOE (G9)")]
    oe_sharpen: Option<OeSharpen>,
    /// PixelOE sharpening amount. Default 0.5.
    #[arg(long, help_heading = "PixelOE (G9)")]
    oe_sharpen_factor: Option<f32>,
    /// Skip PixelOE's colour matching to the input.
    #[arg(long, help_heading = "PixelOE (G9)")]
    oe_no_match: bool,
    /// PixelOE dithering when quantising (-k). Default ordered.
    #[arg(long, value_enum, help_heading = "PixelOE (G9)")]
    oe_dither: Option<OeDither>,
    /// PixelOE quantisation mode (-k). Default kmeans.
    #[arg(long, value_enum, help_heading = "PixelOE (G9)")]
    oe_quant: Option<OeQuant>,
    /// Keep upstream's colour match after quantising; the output then has more than -k colours
    /// (G10).
    #[arg(long, help_heading = "PixelOE (G9)")]
    oe_post_match: bool,
    /// PixelOE outline weight mapping. Default current.
    #[arg(long, value_enum, help_heading = "PixelOE (G9)")]
    oe_mapping: Option<OeMapping>,
    /// PixelOE weight normalisation. Default global.
    #[arg(long, value_enum, help_heading = "PixelOE (G9)")]
    oe_normalize: Option<OeNormalize>,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum MethodArg {
    Pia,
    Pixeloe,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum OeMode {
    Contrast,
    KCentroid,
    Lanczos,
    Nearest,
    NearestExact,
    Bilinear,
    Bicubic,
    Area,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum OeSharpen {
    Unsharp,
    Laplacian,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum OeDither {
    Ordered,
    ErrorDiffusion,
    None,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum OeQuant {
    Kmeans,
    WeightedKmeans,
    RepeatKmeans,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum OeMapping {
    Current,
    ContrastRatio,
    ContrastGated,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum OeNormalize {
    Global,
    None,
}

impl Cli {
    fn uses_oe_flags(&self) -> bool {
        self.oe_thickness.is_some()
            || self.oe_mode.is_some()
            || self.oe_sharpen.is_some()
            || self.oe_sharpen_factor.is_some()
            || self.oe_no_match
            || self.oe_dither.is_some()
            || self.oe_quant.is_some()
            || self.oe_post_match
            || self.oe_mapping.is_some()
            || self.oe_normalize.is_some()
    }

    fn pixeloe_options(&self, pixel_size: usize) -> Options {
        let d = Options::default();
        Options {
            pixel_size,
            thickness: self.oe_thickness.unwrap_or(d.thickness),
            mode: match self.oe_mode {
                None => d.mode,
                Some(OeMode::Contrast) => Mode::Contrast,
                Some(OeMode::KCentroid) => Mode::KCentroid,
                Some(OeMode::Lanczos) => Mode::Lanczos,
                Some(OeMode::Nearest) => Mode::Interp(Interp::Nearest),
                Some(OeMode::NearestExact) => Mode::Interp(Interp::NearestExact),
                Some(OeMode::Bilinear) => Mode::Interp(Interp::Bilinear),
                Some(OeMode::Bicubic) => Mode::Interp(Interp::Bicubic),
                Some(OeMode::Area) => Mode::Interp(Interp::Area),
            },
            sharpen: self.oe_sharpen.map(|s| match s {
                OeSharpen::Unsharp => Sharpen::Unsharp,
                OeSharpen::Laplacian => Sharpen::Laplacian,
            }),
            sharpen_factor: self.oe_sharpen_factor.unwrap_or(d.sharpen_factor),
            color_match: !self.oe_no_match,
            quant: self.colors,
            quant_mode: match self.oe_quant {
                None => d.quant_mode,
                Some(OeQuant::Kmeans) => Quant::Kmeans,
                Some(OeQuant::WeightedKmeans) => Quant::WeightedKmeans,
                Some(OeQuant::RepeatKmeans) => Quant::RepeatKmeans,
            },
            dither: match self.oe_dither {
                None => d.dither,
                Some(OeDither::Ordered) => Dither::Ordered,
                Some(OeDither::ErrorDiffusion) => Dither::ErrorDiffusion,
                Some(OeDither::None) => Dither::None,
            },
            post_match: self.oe_post_match,
            post_upscale: false,
            mapping: match self.oe_mapping {
                None => d.mapping,
                Some(OeMapping::Current) => Mapping::Current,
                Some(OeMapping::ContrastRatio) => Mapping::ContrastRatio,
                Some(OeMapping::ContrastGated) => Mapping::ContrastGated,
            },
            normalize: match self.oe_normalize {
                None => d.normalize,
                Some(OeNormalize::Global) => Normalize::Global,
                Some(OeNormalize::None) => Normalize::None,
            },
        }
    }
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum SpaceArg {
    Oklab,
    Cielab,
}

fn load_gray(path: &PathBuf, w: u32, h: u32) -> Result<Vec<F>, Box<dyn std::error::Error>> {
    let mut g: GrayImage = image::open(path)?.to_luma8();
    if g.dimensions() != (w, h) {
        g = image::imageops::resize(&g, w, h, FilterType::Triangle);
    }
    Ok(g.pixels().map(|p| p.0[0] as F / 255.0).collect())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.method {
        MethodArg::Pia if cli.uses_oe_flags() => {
            return Err("--oe-* flags need --method pixeloe (G9)".into());
        }
        MethodArg::Pixeloe if cli.importance.is_some() || cli.pyxeled_compat => {
            return Err("--importance and --pyxeled-compat are PIA-only".into());
        }
        _ => {}
    }
    if let Some(t) = cli.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(t)
            .build_global()?;
    }
    let t_wall = Instant::now();
    let mut img: RgbImage = image::open(&cli.input)?.to_rgb8();
    let (iw, ih) = img.dimensions();

    let (ow, oh) = match (cli.width, cli.height) {
        (Some(w), Some(h)) => (w, h),
        _ => {
            let s = cli.size as f64;
            if iw >= ih {
                (
                    cli.size,
                    ((ih as f64 * s / iw as f64).round() as usize).max(1),
                )
            } else {
                (
                    ((iw as f64 * s / ih as f64).round() as usize).max(1),
                    cli.size,
                )
            }
        }
    };

    if cli.method == MethodArg::Pixeloe {
        return run_pixeloe(&cli, img, ow, oh, t_wall);
    }

    if cli.input_factor > 0.0 {
        let f = cli.input_factor;
        let (mw, mh) = (
            (ow as f64 * f).round() as u32,
            (oh as f64 * f).round() as u32,
        );
        if iw > mw || ih > mh {
            img = image::imageops::resize(&img, mw, mh, FilterType::Lanczos3);
        }
    }
    img = enlarge_to_cover(img, ow, oh);
    let (w, h) = img.dimensions();
    let rgb: Vec<[F; 3]> = img.pixels().map(|p| p.0.map(|v| v as F / 255.0)).collect();
    let importance = match cli.importance.as_ref() {
        None => None,
        Some(p) if p.as_os_str() == "auto" => {
            let rgb8: Vec<[u8; 3]> = img.pixels().map(|p| p.0).collect();
            let map = spectral_residual(&rgb8, w as usize, h as usize);
            Some(map.iter().map(|&v| v as F / 255.0).collect::<Vec<F>>())
        }
        Some(p) => Some(load_gray(p, w, h)?),
    };

    let mut p = Params::new(ow, oh, cli.colors.unwrap_or(16));
    p.space = match cli.space {
        SpaceArg::Oklab => Space::Oklab,
        SpaceArg::Cielab => Space::Cielab,
    };
    p.saturation = cli.saturation;
    p.t0 = cli.t0;
    p.m = cli.m;
    p.max_iters = cli.max_iters;
    p.pyxeled_compat = cli.pyxeled_compat;

    let input = Input {
        width: w as usize,
        height: h as usize,
        rgb: &rgb,
        importance: importance.as_deref(),
    };
    let out = run(&input, &p);

    let png = RgbImage::from_raw(out.width as u32, out.height as u32, out.rgb8()).unwrap();
    png.save(&cli.output)?;
    if let Some(f) = cli.preview {
        let big =
            image::imageops::resize(&png, png.width() * f, png.height() * f, FilterType::Nearest);
        big.save(cli.output.with_extension(format!("x{f}.png")))?;
    }

    let s = &out.stats;
    eprintln!(
        "{}x{} -> {}x{}  K={}  iters={} (T-steps {}){}  solve {:.0} ms  [assign {:.0} | accum {:.0} | smooth {:.0} | mcda {:.0}]  peak RSS {} MiB",
        w, h, out.width, out.height, s.palette_size, s.iterations, s.temperature_steps,
        if s.hit_max_iters { " HIT MAX_ITERS" } else { "" },
        s.timings.total_ms, s.timings.assign_ms, s.timings.accumulate_ms, s.timings.smooth_ms, s.timings.associate_ms,
        peak_rss_kib().unwrap_or(0) / 1024
    );

    if let Some(path) = cli.stats {
        let j = serde_json::json!({
            "params": p,
            "stats": s,
            "palette_srgb": out.palette_srgb,
            "wall_ms": t_wall.elapsed().as_secs_f64() * 1000.0,
            "peak_rss_kib": peak_rss_kib(),
            "threads": rayon::current_num_threads(),
        });
        std::fs::write(path, serde_json::to_string_pretty(&j)?)?;
    }
    Ok(())
}

/// I1: PIA needs at least one input pixel per output pixel. A smaller input is enlarged
/// (Lanczos, aspect kept) just enough to cover the output grid; larger inputs are untouched.
fn enlarge_to_cover(img: RgbImage, ow: usize, oh: usize) -> RgbImage {
    let (w, h) = img.dimensions();
    if w as usize >= ow && h as usize >= oh {
        return img;
    }
    let scale = (ow as f64 / w as f64).max(oh as f64 / h as f64);
    let nw = ((w as f64 * scale).ceil() as u32).max(ow as u32);
    let nh = ((h as f64 * scale).ceil() as u32).max(oh as u32);
    image::imageops::resize(&img, nw, nh, FilterType::Lanczos3)
}

/// `--method pixeloe` (G4, G7, G8): same output files and `--stats` keys as PIA.
fn run_pixeloe(
    cli: &Cli,
    mut img: RgbImage,
    ow: usize,
    oh: usize,
    t_wall: Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    let (iw, ih) = img.dimensions();
    let ps = if cli.input_factor > 0.0 {
        let ps = (cli.input_factor.round() as usize).max(1);
        let (tw, th) = ((ow * ps) as u32, (oh * ps) as u32);
        if (iw, ih) != (tw, th) {
            img = image::imageops::resize(&img, tw, th, FilterType::Lanczos3);
        }
        ps
    } else {
        ((iw.max(ih) as f64 / cli.size as f64).round() as usize).max(1)
    };
    let (w, h) = (img.width() as usize, img.height() as usize);
    let n = w * h;
    let mut data = vec![0.0f32; 3 * n];
    for (i, p) in img.pixels().enumerate() {
        for c in 0..3 {
            data[c * n + i] = p.0[c] as f32 / 255.0;
        }
    }
    let opts = cli.pixeloe_options(ps);
    let t = Instant::now();
    let out = pixelize(&Img::from_vec(3, h, w, data), &opts);
    let total_ms = t.elapsed().as_secs_f64() * 1000.0;

    let m = out.h * out.w;
    let rgb8: Vec<u8> = (0..m)
        .flat_map(|i| (0..3).map(move |c| (c, i)))
        .map(|(c, i)| (out.data[c * m + i].clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    let mut palette: Vec<[u8; 3]> = rgb8.as_chunks::<3>().0.to_vec();
    palette.sort_unstable();
    palette.dedup();

    let png = RgbImage::from_raw(out.w as u32, out.h as u32, rgb8).unwrap();
    png.save(&cli.output)?;
    if let Some(f) = cli.preview {
        let big =
            image::imageops::resize(&png, png.width() * f, png.height() * f, FilterType::Nearest);
        big.save(cli.output.with_extension(format!("x{f}.png")))?;
    }
    eprintln!(
        "pixeloe {}x{} -> {}x{}  pixel size {}  colours {}  solve {:.0} ms  peak RSS {} MiB",
        w,
        h,
        out.w,
        out.h,
        ps,
        palette.len(),
        total_ms,
        peak_rss_kib().unwrap_or(0) / 1024
    );

    if let Some(path) = &cli.stats {
        let mut s = Stats {
            palette_size: palette.len(),
            input_w: w,
            input_h: h,
            ..Stats::default()
        };
        s.timings.total_ms = total_ms;
        let j = serde_json::json!({
            "params": {
                "method": "pixeloe",
                "width": out.w,
                "height": out.h,
                "pixel_size": opts.pixel_size,
                "thickness": opts.thickness,
                "mode": format!("{:?}", opts.mode),
                "sharpen": opts.sharpen.map(|s| format!("{s:?}")),
                "sharpen_factor": opts.sharpen_factor,
                "color_match": opts.color_match,
                "k_max": opts.quant,
                "quant_mode": format!("{:?}", opts.quant_mode),
                "dither": format!("{:?}", opts.dither),
                "post_match": opts.post_match,
                "mapping": format!("{:?}", opts.mapping),
                "normalize": format!("{:?}", opts.normalize),
            },
            "stats": s,
            "palette_srgb": palette,
            "wall_ms": t_wall.elapsed().as_secs_f64() * 1000.0,
            "peak_rss_kib": peak_rss_kib(),
            "threads": rayon::current_num_threads(),
        });
        std::fs::write(path, serde_json::to_string_pretty(&j)?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enlarge_to_cover_only_grows_small_inputs() {
        let small = RgbImage::new(100, 80);
        assert_eq!(
            enlarge_to_cover(small.clone(), 128, 102).dimensions(),
            (128, 103)
        );
        assert_eq!(
            enlarge_to_cover(small.clone(), 50, 300).dimensions(),
            (375, 300)
        );
        assert_eq!(enlarge_to_cover(small, 100, 80).dimensions(), (100, 80));
        let tiny = RgbImage::new(1, 1);
        assert_eq!(enlarge_to_cover(tiny, 16, 9).dimensions(), (16, 16));
    }
}
