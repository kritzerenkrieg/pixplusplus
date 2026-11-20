use clap::Parser;
use image::RgbImage;
use pixplusplus::decimate::Distance;
use pixplusplus::job::{
    output_size, pia_image, pixeloe_defaults, pixeloe_image, upscale, Importance,
};
use pixplusplus::pixeloe::color::Dither;
use pixplusplus::pixeloe::downscale::Interp;
use pixplusplus::pixeloe::outline::{Mapping, Normalize};
use pixplusplus::pixeloe::pipeline::{Mode, Options, Quant, Sharpen};
use pixplusplus::{peak_rss_kib, Adjust, ColorAdjust, Params, Space, Stats, F};
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
    /// How the -k palette is made (K1). `native`: the method's own (PIA's solver, PixelOE's
    /// k-means). `decimate`: merge the closest colours (RGB), keeping the more common one,
    /// then map each pixel to its nearest colour, no dither. Under PixelOE it needs -k.
    #[arg(long, value_enum, default_value_t = PaletteArg::Native)]
    palette: PaletteArg,
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
    /// Output saturation, chroma × this (K4). Default: PIA 1.1 (the paper's palette boost, B5),
    /// PixelOE 1.0.
    #[arg(long, value_parser = parse_factor)]
    saturation: Option<F>,
    /// Output contrast: lightness stretched around the image's mean lightness (K4). Never adds
    /// colours.
    #[arg(long, default_value_t = 1.0, value_parser = parse_factor)]
    contrast: F,
    /// Input saturation, applied before the palette is chosen (K4).
    #[arg(long, default_value_t = 1.0, value_parser = parse_factor)]
    pre_saturation: F,
    /// Input contrast, applied before the palette is chosen (K4).
    #[arg(long, default_value_t = 1.0, value_parser = parse_factor)]
    pre_contrast: F,
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

    /// PixelOE outline thickness, 0-6 (0 = no outline expansion). Default 3.
    #[arg(long, value_parser = parse_thickness, help_heading = "PixelOE (G9)")]
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
enum PaletteArg {
    Native,
    Decimate,
}

impl PaletteArg {
    /// K1: decimation measures distance in RGB, the user's proposal.
    fn distance(self) -> Option<Distance> {
        (self == PaletteArg::Decimate).then_some(Distance::Rgb)
    }
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

/// Saturation and contrast factors (K4): finite and not negative.
fn parse_factor(s: &str) -> Result<F, String> {
    match s.parse::<F>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Ok(v),
        _ => Err("expected a number >= 0 (1 = unchanged)".into()),
    }
}

/// PixelOE has soft circle kernels for thickness 1..=6 only (`morph::kernel`).
fn parse_thickness(s: &str) -> Result<usize, String> {
    match s.parse::<usize>() {
        Ok(t) if t <= 6 => Ok(t),
        _ => Err("expected a whole number from 0 to 6".into()),
    }
}

impl Cli {
    /// K4 adjustments. `post_saturation` is the method's own default when not given.
    fn color_adjust(&self, post_saturation: F) -> ColorAdjust {
        ColorAdjust {
            pre: Adjust {
                saturation: self.pre_saturation,
                contrast: self.pre_contrast,
            },
            post: Adjust {
                saturation: post_saturation,
                contrast: self.contrast,
            },
        }
    }

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
        let d = pixeloe_defaults();
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
    if cli.palette == PaletteArg::Decimate {
        if cli.pyxeled_compat {
            return Err("--palette decimate can't be combined with --pyxeled-compat (A4)".into());
        }
        if cli.method == MethodArg::Pixeloe && cli.colors.is_none() {
            return Err("--palette decimate needs -k under --method pixeloe (G8)".into());
        }
    }
    if let Some(t) = cli.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(t)
            .build_global()?;
    }
    let t_wall = Instant::now();
    let img: RgbImage = image::open(&cli.input)?.to_rgb8();
    let (iw, ih) = img.dimensions();
    let (ow, oh) = output_size(iw, ih, cli.size, cli.width, cli.height);

    if cli.method == MethodArg::Pixeloe {
        return run_pixeloe(&cli, img, ow, oh, t_wall);
    }

    let importance = match cli.importance.as_ref() {
        None => Importance::None,
        Some(p) if p.as_os_str() == "auto" => Importance::Auto,
        Some(p) => Importance::Map(image::open(p)?.to_luma8()),
    };
    let mut p = Params::new(ow, oh, cli.colors.unwrap_or(16));
    p.space = match cli.space {
        SpaceArg::Oklab => Space::Oklab,
        SpaceArg::Cielab => Space::Cielab,
    };
    // PIA's output saturation is the solver's palette boost (B5); the adjust step does contrast.
    p.saturation = cli.saturation.unwrap_or(1.1);
    p.t0 = cli.t0;
    p.m = cli.m;
    p.max_iters = cli.max_iters;
    p.pyxeled_compat = cli.pyxeled_compat;

    let r = pia_image(
        img,
        p,
        cli.input_factor,
        &importance,
        &cli.color_adjust(1.0),
        cli.palette.distance(),
    );
    if let Some(w) = &r.warning {
        eprintln!("warning: {w}");
    }
    let (out, p, png) = (&r.output, &r.params, &r.image);
    png.save(&cli.output)?;
    if let Some(f) = cli.preview {
        upscale(png, f).save(cli.output.with_extension(format!("x{f}.png")))?;
    }

    let s = &out.stats;
    eprintln!(
        "{}x{} -> {}x{}  K={}  iters={} (T-steps {}){}  solve {:.0} ms  [assign {:.0} | accum {:.0} | smooth {:.0} | mcda {:.0}]  peak RSS {} MiB",
        r.input_w, r.input_h, out.width, out.height, s.palette_size, s.iterations, s.temperature_steps,
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

/// `--method pixeloe` (G4, G7, G8): same output files and `--stats` keys as PIA.
fn run_pixeloe(
    cli: &Cli,
    img: RgbImage,
    ow: usize,
    oh: usize,
    t_wall: Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    let r = pixeloe_image(
        img,
        ow,
        oh,
        cli.size,
        cli.input_factor,
        cli.pixeloe_options(0),
        &cli.color_adjust(cli.saturation.unwrap_or(1.0)),
        cli.palette.distance(),
    );
    let (png, opts, palette) = (&r.image, &r.options, &r.palette);
    png.save(&cli.output)?;
    if let Some(f) = cli.preview {
        upscale(png, f).save(cli.output.with_extension(format!("x{f}.png")))?;
    }
    eprintln!(
        "pixeloe {}x{} -> {}x{}  pixel size {}  colours {}  solve {:.0} ms  peak RSS {} MiB",
        r.input_w,
        r.input_h,
        png.width(),
        png.height(),
        opts.pixel_size,
        palette.len(),
        r.solve_ms,
        peak_rss_kib().unwrap_or(0) / 1024
    );

    if let Some(path) = &cli.stats {
        let mut s = Stats {
            palette_size: palette.len(),
            input_w: r.input_w,
            input_h: r.input_h,
            ..Stats::default()
        };
        s.timings.total_ms = r.solve_ms;
        let j = serde_json::json!({
            "params": {
                "method": "pixeloe",
                "width": png.width(),
                "height": png.height(),
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
