//! Saturation and contrast (K4), in Oklab.
//!
//! Saturation multiplies chroma (a, b). Contrast stretches lightness around the image's mean
//! lightness, so overall brightness stays put: `L' = mean + (L - mean) * contrast`. Out-of-gamut
//! results are clamped per sRGB channel.
//!
//! The adjustment is a function of the colour alone (the mean is fixed per image), so applied
//! to an output it never adds colours and `-k` stays a hard limit (G10). Neutral values (1, 1)
//! leave the image untouched.

use crate::color::{oklab_to_srgb, srgb_to_oklab};
use crate::F;
use image::RgbImage;
use std::collections::HashMap;

/// One saturation/contrast adjustment. Used twice per run: before the palette is chosen (pre)
/// and on the final output (post).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Adjust {
    /// Chroma multiplier. 1 = unchanged, 0 = grey.
    pub saturation: F,
    /// Lightness stretch around the mean lightness. 1 = unchanged, 0 = flat.
    pub contrast: F,
}

impl Default for Adjust {
    fn default() -> Self {
        Self {
            saturation: 1.0,
            contrast: 1.0,
        }
    }
}

/// The two adjustments of a run (K4).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColorAdjust {
    pub pre: Adjust,
    pub post: Adjust,
}

impl Adjust {
    pub fn is_neutral(&self) -> bool {
        self.saturation == 1.0 && self.contrast == 1.0
    }

    /// The per-colour map for `img`: each distinct colour and its adjusted value. Empty when
    /// neutral.
    pub fn colour_map(&self, img: &RgbImage) -> HashMap<[u8; 3], [u8; 3]> {
        let mut map = HashMap::new();
        if self.is_neutral() || img.width() == 0 || img.height() == 0 {
            return map;
        }
        let mut sum_l = 0.0f64;
        let mut labs: HashMap<[u8; 3], [F; 3]> = HashMap::new();
        for p in img.pixels() {
            let lab = *labs
                .entry(p.0)
                .or_insert_with(|| srgb_to_oklab(p.0.map(|v| v as F / 255.0)));
            sum_l += lab[0] as f64;
        }
        let mean = (sum_l / (img.width() as f64 * img.height() as f64)) as F;
        for (rgb, [l, a, b]) in labs {
            let lab = [
                mean + (l - mean) * self.contrast,
                a * self.saturation,
                b * self.saturation,
            ];
            let out = oklab_to_srgb(lab).map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
            map.insert(rgb, out);
        }
        map
    }

    /// Adjusts `img` in place.
    pub fn apply(&self, img: &mut RgbImage) {
        let map = self.colour_map(img);
        if map.is_empty() {
            return;
        }
        for p in img.pixels_mut() {
            p.0 = map[&p.0];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn sample() -> RgbImage {
        RgbImage::from_fn(24, 16, |x, y| {
            image::Rgb([(x * 10) as u8, (y * 15) as u8, ((x + y) * 6) as u8])
        })
    }

    fn colours(img: &RgbImage) -> usize {
        img.pixels().map(|p| p.0).collect::<HashSet<_>>().len()
    }

    fn mean_l(img: &RgbImage) -> F {
        let n = (img.width() * img.height()) as F;
        img.pixels()
            .map(|p| srgb_to_oklab(p.0.map(|v| v as F / 255.0))[0])
            .sum::<F>()
            / n
    }

    #[test]
    fn neutral_is_a_no_op() {
        let img = sample();
        let mut out = img.clone();
        Adjust::default().apply(&mut out);
        assert_eq!(out, img);
    }

    #[test]
    fn never_adds_colours() {
        let img = sample();
        for (s, c) in [(1.5, 1.0), (0.5, 1.4), (2.0, 0.6), (0.0, 0.0)] {
            let mut out = img.clone();
            Adjust {
                saturation: s,
                contrast: c,
            }
            .apply(&mut out);
            assert!(colours(&out) <= colours(&img), "s={s} c={c}");
        }
    }

    #[test]
    fn zero_saturation_is_grey() {
        let mut out = sample();
        Adjust {
            saturation: 0.0,
            contrast: 1.0,
        }
        .apply(&mut out);
        for p in out.pixels() {
            let [r, g, b] = p.0.map(|v| v as i32);
            assert!((r - g).abs() <= 1 && (g - b).abs() <= 1, "{:?}", p.0);
        }
    }

    #[test]
    fn contrast_keeps_mean_lightness_and_spreads_it() {
        // Mid-tones only, so the stretch stays inside the gamut.
        let img = RgbImage::from_fn(16, 16, |x, y| {
            let v = 90 + ((x + y) * 3) as u8;
            image::Rgb([v, v, v])
        });
        let mut out = img.clone();
        Adjust {
            saturation: 1.0,
            contrast: 1.5,
        }
        .apply(&mut out);
        assert!((mean_l(&out) - mean_l(&img)).abs() < 0.01);
        let spread = |im: &RgbImage| {
            let ls: Vec<F> = im
                .pixels()
                .map(|p| srgb_to_oklab(p.0.map(|v| v as F / 255.0))[0])
                .collect();
            ls.iter().cloned().fold(F::MIN, F::max) - ls.iter().cloned().fold(F::MAX, F::min)
        };
        assert!(spread(&out) > spread(&img) * 1.3);
    }
}
