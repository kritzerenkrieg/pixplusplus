//! K1 comparison: native palette vs decimation with RGB and Oklab distance, both methods.
//!
//! `cargo run --release --example decimate_compare SRC_DIR OUT_DIR [SIZE]`
//! Writes `<image file name>_<method>_k<k>_<variant>.png` and prints one line per run with its time.
//! Light at 128 (a few seconds per image); larger sizes belong on a remote machine (A5).
use pixplusplus::decimate::Distance;
use pixplusplus::job::{output_size, pia_image, pixeloe_defaults, pixeloe_image, Importance};
use pixplusplus::{ColorAdjust, Params};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let src = PathBuf::from(args.next().expect("SRC_DIR"));
    let out = PathBuf::from(args.next().expect("OUT_DIR"));
    let size: usize = args.next().map_or(128, |s| s.parse().expect("SIZE"));
    std::fs::create_dir_all(&out).unwrap();
    let mut imgs: Vec<PathBuf> = std::fs::read_dir(&src)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                ["png", "jpg", "jpeg", "webp"].contains(&e.to_lowercase().as_str())
            })
        })
        .collect();
    imgs.sort();
    let variants = [
        ("native", None),
        ("rgb", Some(Distance::Rgb)),
        ("oklab", Some(Distance::Oklab)),
    ];
    let adjust = ColorAdjust::default();
    for path in imgs {
        // The full file name, so scene.jpg and scene.png don't overwrite each other.
        let stem = path.file_name().unwrap().to_string_lossy().into_owned();
        let img = image::open(&path).unwrap().to_rgb8();
        let (ow, oh) = output_size(img.width(), img.height(), size, None, None);
        for k in [8, 16] {
            for (name, d) in variants {
                let t = Instant::now();
                let r = pia_image(
                    img.clone(),
                    Params::new(ow, oh, k),
                    4.0,
                    &Importance::None,
                    &adjust,
                    d,
                );
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                r.image
                    .save(out.join(format!("{stem}_pia_k{k}_{name}.png")))
                    .unwrap();
                println!("{stem} pia k{k} {name} {ms:.0} ms");

                let mut o = pixeloe_defaults();
                o.quant = Some(k);
                let t = Instant::now();
                let r = pixeloe_image(img.clone(), ow, oh, size, 4.0, o, &adjust, d);
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                r.image
                    .save(out.join(format!("{stem}_pixeloe_k{k}_{name}.png")))
                    .unwrap();
                println!("{stem} pixeloe k{k} {name} {ms:.0} ms");
            }
        }
    }
}
