//! CIELAB (D65, 2°) as PixelOE uses it: kornia's `rgb_to_lab` / `lab_to_rgb` (kornia 0.9.2,
//! `kornia/color/{lab,xyz,rgb}.py`). PixelOE prefers kornia over its own fallback whenever
//! kornia is installed, and kornia is a hard dependency of pixeloe 1.0.0.
//!
//! This is separate from `crate::color`'s CIELAB (skimage constants, Pyxeled parity): the
//! matrices and the linear-segment constants differ slightly.

use super::Img;

/// kornia `rgb_to_xyz` matrix (sRGB D65).
const RGB_TO_XYZ: [[f32; 3]; 3] = [
    [0.412453, 0.357580, 0.180423],
    [0.212671, 0.715160, 0.072169],
    [0.019334, 0.119193, 0.950227],
];
/// kornia `xyz_to_rgb` matrix.
#[allow(clippy::excessive_precision)]
const XYZ_TO_RGB: [[f32; 3]; 3] = [
    [3.2404813432005266, -1.5371515162713185, -0.4985363261688878],
    [-0.9692549499965682, 1.8759900014898907, 0.0415559265582928],
    [0.0556466391351772, -0.2040413383665112, 1.0573110696453443],
];
/// D65 reference white.
const WHITE: [f32; 3] = [0.95047, 1.0, 1.08883];

#[inline]
fn srgb_to_linear(v: f32) -> f32 {
    if v > 0.04045 {
        ((v + 0.055).max(0.0) / 1.055).powf(2.4)
    } else {
        v / 12.92
    }
}

#[inline]
fn linear_to_srgb(v: f32) -> f32 {
    if v > 0.003_130_8 {
        1.055 * v.max(0.003_130_8).powf(1.0 / 2.4) - 0.055
    } else {
        12.92 * v
    }
}

/// One pixel, sRGB 0..1 -> L 0..100, a, b.
#[inline]
pub fn rgb_to_lab_px(rgb: [f32; 3]) -> [f32; 3] {
    let lin = rgb.map(srgb_to_linear);
    let mut f = [0.0f32; 3];
    for (i, row) in RGB_TO_XYZ.iter().enumerate() {
        let v = (row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2]) / WHITE[i];
        let t = 0.008856;
        f[i] = if v > t {
            v.max(t).powf(1.0 / 3.0)
        } else {
            7.787 * v + 4.0 / 29.0
        };
    }
    [
        116.0 * f[1] - 16.0,
        500.0 * (f[0] - f[1]),
        200.0 * (f[1] - f[2]),
    ]
}

/// One pixel, Lab -> sRGB clamped to 0..1.
#[inline]
pub fn lab_to_rgb_px(lab: [f32; 3]) -> [f32; 3] {
    let fy = (lab[0] + 16.0) / 116.0;
    let fx = lab[1] / 500.0 + fy;
    let fz = (fy - lab[2] / 200.0).max(0.0);
    let xyz = [fx, fy, fz].map(|f| {
        if f > 0.206_896_6 {
            f.powf(3.0)
        } else {
            (f - 4.0 / 29.0) / 7.787
        }
    });
    let xyz = [xyz[0] * WHITE[0], xyz[1] * WHITE[1], xyz[2] * WHITE[2]];
    let mut rgb = [0.0f32; 3];
    for (i, row) in XYZ_TO_RGB.iter().enumerate() {
        let lin = row[0] * xyz[0] + row[1] * xyz[1] + row[2] * xyz[2];
        rgb[i] = linear_to_srgb(lin).clamp(0.0, 1.0);
    }
    rgb
}

fn per_pixel(img: &Img, f: impl Fn([f32; 3]) -> [f32; 3]) -> Img {
    assert_eq!(img.c, 3);
    let n = img.h * img.w;
    let mut out = Img::new(3, img.h, img.w);
    for i in 0..n {
        let v = f([img.data[i], img.data[n + i], img.data[2 * n + i]]);
        out.data[i] = v[0];
        out.data[n + i] = v[1];
        out.data[2 * n + i] = v[2];
    }
    out
}

/// kornia `rgb_to_lab` on a 3-channel image.
pub fn rgb_to_lab(img: &Img) -> Img {
    per_pixel(img, rgb_to_lab_px)
}

/// kornia `lab_to_rgb` (clip = True) on a 3-channel image.
pub fn lab_to_rgb(img: &Img) -> Img {
    per_pixel(img, lab_to_rgb_px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixeloe::fixtures::{assert_close, img};

    #[test]
    fn lab_matches_kornia() {
        let a = img("a_img");
        assert_close("a_lab", &rgb_to_lab(&a), 2e-4);
        assert_close("a_lab_to_rgb", &lab_to_rgb(&img("a_lab")), 1e-5);
        assert_close("a_wild_lab_to_rgb", &lab_to_rgb(&img("a_wild_lab")), 1e-5);
    }
}
