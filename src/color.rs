//! Colour space conversions.
//!
//! The working space is Oklab scaled by [`OKLAB_SCALE`] (L in 0..100, a/b roughly ±40),
//! so the paper's CIELAB-tuned constants (T0 = 35, m = 45, ε_palette = 1, …) keep the
//! same order of magnitude. CIELAB is only here for the Pyxeled-compat correctness mode.

pub type F = f32;

/// Oklab is multiplied by this inside the solver. Reported ΔE values divide it back out.
pub const OKLAB_SCALE: F = 100.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Space {
    Oklab,
    /// CIELAB D65 — matches skimage.color.rgb2lab, used only for Pyxeled parity checks.
    Cielab,
}

#[inline]
fn srgb_to_linear(c: F) -> F {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn linear_to_srgb(c: F) -> F {
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB (0..1) -> Oklab (unscaled).
pub fn srgb_to_oklab(rgb: [F; 3]) -> [F; 3] {
    let [r, g, b] = rgb.map(srgb_to_linear);
    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Oklab (unscaled) -> sRGB (0..1, unclamped).
pub fn oklab_to_srgb(lab: [F; 3]) -> [F; 3] {
    let [ll, a, b] = lab;
    let l = ll + 0.396_337_78 * a + 0.215_803_76 * b;
    let m = ll - 0.105_561_346 * a - 0.063_854_17 * b;
    let s = ll - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let bb = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
    [r, g, bb].map(linear_to_srgb)
}

const XN: F = 0.950_47;
const YN: F = 1.0;
const ZN: F = 1.088_83;

fn lab_f(t: F) -> F {
    if t > 0.008_856 {
        t.cbrt()
    } else {
        7.787 * t + 16.0 / 116.0
    }
}

fn lab_finv(t: F) -> F {
    let t3 = t * t * t;
    if t3 > 0.008_856 {
        t3
    } else {
        (t - 16.0 / 116.0) / 7.787
    }
}

pub fn srgb_to_cielab(rgb: [F; 3]) -> [F; 3] {
    let [r, g, b] = rgb.map(srgb_to_linear);
    let x = 0.412_453 * r + 0.357_580 * g + 0.180_423 * b;
    let y = 0.212_671 * r + 0.715_160 * g + 0.072_169 * b;
    let z = 0.019_334 * r + 0.119_193 * g + 0.950_227 * b;
    let (fx, fy, fz) = (lab_f(x / XN), lab_f(y / YN), lab_f(z / ZN));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

pub fn cielab_to_srgb(lab: [F; 3]) -> [F; 3] {
    let fy = (lab[0] + 16.0) / 116.0;
    let fx = fy + lab[1] / 500.0;
    let fz = fy - lab[2] / 200.0;
    let (x, y, z) = (lab_finv(fx) * XN, lab_finv(fy) * YN, lab_finv(fz) * ZN);
    let r = 3.240_481 * x - 1.537_151 * y - 0.498_536 * z;
    let g = -0.969_255 * x + 1.875_99 * y + 0.041_556 * z;
    let b = 0.055_647 * x - 0.204_041 * y + 1.057_311 * z;
    [r, g, b].map(|c| linear_to_srgb(c.max(0.0)))
}

impl Space {
    /// sRGB 0..1 -> working space (Oklab is scaled by OKLAB_SCALE).
    pub fn from_srgb(self, rgb: [F; 3]) -> [F; 3] {
        match self {
            Space::Oklab => srgb_to_oklab(rgb).map(|v| v * OKLAB_SCALE),
            Space::Cielab => srgb_to_cielab(rgb),
        }
    }

    /// Working space -> sRGB 0..1, clamped.
    pub fn to_srgb(self, c: [F; 3]) -> [F; 3] {
        let rgb = match self {
            Space::Oklab => oklab_to_srgb(c.map(|v| v / OKLAB_SCALE)),
            Space::Cielab => cielab_to_srgb(c),
        };
        rgb.map(|v| v.clamp(0.0, 1.0))
    }
}

#[inline]
pub fn dist(a: &[F; 3], b: &[F; 3]) -> F {
    let d0 = a[0] - b[0];
    let d1 = a[1] - b[1];
    let d2 = a[2] - b[2];
    (d0 * d0 + d1 * d1 + d2 * d2).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklab_roundtrip_and_white() {
        let w = srgb_to_oklab([1.0, 1.0, 1.0]);
        assert!((w[0] - 1.0).abs() < 1e-3 && w[1].abs() < 1e-3 && w[2].abs() < 1e-3);
        for rgb in [[0.2, 0.5, 0.9], [1.0, 0.0, 0.0], [0.05, 0.05, 0.05]] {
            let back = oklab_to_srgb(srgb_to_oklab(rgb));
            for i in 0..3 {
                assert!((back[i] - rgb[i]).abs() < 1e-3, "{rgb:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn cielab_matches_reference() {
        // skimage.color.rgb2lab([[[1,0,0]]]) = [53.24, 80.09, 67.20]
        let lab = srgb_to_cielab([1.0, 0.0, 0.0]);
        assert!((lab[0] - 53.24).abs() < 0.05);
        assert!((lab[1] - 80.09).abs() < 0.1);
        assert!((lab[2] - 67.20).abs() < 0.1);
        let back = cielab_to_srgb(lab);
        assert!((back[0] - 1.0).abs() < 1e-3 && back[1].abs() < 1e-2);
    }
}
