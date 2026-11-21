//! Per-stage timings of the PixelOE port (G11): `cargo run --release --example oe_profile IMG`.
use pixplusplus::pixeloe::color::match_color;
use pixplusplus::pixeloe::downscale::contrast;
use pixplusplus::pixeloe::outline::{outline_expansion, Mapping, Normalize};
use pixplusplus::pixeloe::Img;
use std::time::Instant;

fn main() {
    let p = std::env::args().nth(1).expect("image path");
    let im = image::open(p).unwrap().to_rgb8();
    let (w, h) = (im.width() as usize, im.height() as usize);
    let n = w * h;
    let mut d = vec![0.0f32; 3 * n];
    for (i, px) in im.pixels().enumerate() {
        for c in 0..3 {
            d[c * n + i] = px.0[c] as f32 / 255.0;
        }
    }
    let img = Img::from_vec(3, h, w, d);
    let t = Instant::now();
    let w8 = pixplusplus::pixeloe::outline::expansion_weight(
        &img,
        4,
        2,
        Mapping::Current,
        Normalize::Global,
    );
    eprintln!(
        "expansion_weight {:7.0} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    let t = Instant::now();
    let (e, _) = outline_expansion(&img, 3, 3, 4, Mapping::Current, Normalize::Global);
    eprintln!(
        "outline (incl. weight) {:7.0} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    let t = Instant::now();
    let m = match_color(&e, &img);
    eprintln!("match_color {:7.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    let _ = contrast(&m, 4);
    eprintln!("contrast {:7.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let _ = w8;
}
