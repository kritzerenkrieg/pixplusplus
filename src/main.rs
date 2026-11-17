use clap::Parser;
use image::{imageops::FilterType, GrayImage, RgbImage};
use piaru::{peak_rss_kib, run, Input, Params, Space, F};
use std::path::PathBuf;
use std::time::Instant;

/// Concept art -> pixel art via Pixelated Image Abstraction (MCDA in Oklab).
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
    /// Palette size (hard limit).
    #[arg(short = 'k', long, default_value_t = 16)]
    colors: usize,
    /// Downscale the input so it is at most this many times the output (0 = use input as is).
    #[arg(long, default_value_t = 4.0)]
    input_factor: f64,
    #[arg(long, value_enum, default_value_t = SpaceArg::Oklab)]
    space: SpaceArg,
    /// Grayscale importance map (white = more palette mass). Resized to the input.
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
    if let Some(t) = cli.threads {
        rayon::ThreadPoolBuilder::new().num_threads(t).build_global()?;
    }
    let t_wall = Instant::now();
    let mut img: RgbImage = image::open(&cli.input)?.to_rgb8();
    let (iw, ih) = img.dimensions();

    let (ow, oh) = match (cli.width, cli.height) {
        (Some(w), Some(h)) => (w, h),
        _ => {
            let s = cli.size as f64;
            if iw >= ih {
                (cli.size, ((ih as f64 * s / iw as f64).round() as usize).max(1))
            } else {
                (((iw as f64 * s / ih as f64).round() as usize).max(1), cli.size)
            }
        }
    };

    if cli.input_factor > 0.0 {
        let f = cli.input_factor;
        let (mw, mh) = ((ow as f64 * f).round() as u32, (oh as f64 * f).round() as u32);
        if iw > mw || ih > mh {
            img = image::imageops::resize(&img, mw, mh, FilterType::Lanczos3);
        }
    }
    let (w, h) = img.dimensions();
    let rgb: Vec<[F; 3]> = img.pixels().map(|p| p.0.map(|v| v as F / 255.0)).collect();
    let importance = cli.importance.as_ref().map(|p| load_gray(p, w, h)).transpose()?;

    let mut p = Params::new(ow, oh, cli.colors);
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
        let big = image::imageops::resize(&png, png.width() * f, png.height() * f, FilterType::Nearest);
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
