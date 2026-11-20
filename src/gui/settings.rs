//! GUI settings (J3) and the run that turns them into an image through the same glue as the CLI
//! (`pixplusplus::job`, J2). Settings the GUI doesn't show keep the CLI defaults.

use image::RgbImage;
use pixplusplus::decimate::Distance;
use pixplusplus::job::{output_size, pia_image, pixeloe_defaults, pixeloe_image, Importance};
use pixplusplus::pixeloe::color::Dither;
use pixplusplus::pixeloe::pipeline::Options;
use pixplusplus::{Adjust, ColorAdjust, Params, F};
use std::path::PathBuf;
use std::time::Instant;

/// `--input-factor` default (B3, G7); not shown in the GUI (J3).
pub const INPUT_FACTOR: f64 = 4.0;

/// The three methods (E8).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Method {
    Pia,
    PiaImportance,
    Pixeloe,
}

impl Method {
    pub const ALL: [Method; 3] = [Method::Pia, Method::PiaImportance, Method::Pixeloe];

    pub fn label(self) -> &'static str {
        match self {
            Method::Pia => "PIA",
            Method::PiaImportance => "PIA + importance map",
            Method::Pixeloe => "PixelOE",
        }
    }
}

/// How the limited palette is made (K1, K5): the method's own, or decimation in Oklab or RGB.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaletteChoice {
    Native,
    DecimateOklab,
    /// `decimate` is what profiles from the first K1 build wrote; it meant RGB.
    #[serde(alias = "decimate")]
    DecimateRgb,
}

/// Where the importance map comes from (C1, E7).
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportanceSource {
    /// Spectral-residual saliency, `--importance auto` (E7).
    Auto,
    /// A grayscale image, `--importance FILE`.
    File(PathBuf),
}

/// Everything the GUI lets you set; saved as a profile (L1). Missing fields take the defaults,
/// so older profiles keep loading.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub method: Method,
    /// `-s`: output long side.
    pub size: usize,
    /// `-k`: palette size, a hard limit (B4, G10).
    pub colors: usize,
    /// PixelOE quantises only when `-k` is given (G8).
    pub oe_quantise: bool,
    pub importance: ImportanceSource,
    /// `--oe-*` options (G9); `quant` and `pixel_size` are set at run time. A profile with only
    /// some of these fields fills the rest from the GUI defaults, not the library's (L1).
    #[serde(deserialize_with = "oe_over_gui_defaults")]
    pub oe: Options,
    /// `--saturation` under PIA (the solver's palette boost, B5) and under PixelOE (K4). Kept
    /// apart because the defaults differ.
    pub pia_saturation: F,
    pub oe_saturation: F,
    /// `--contrast` (K4).
    pub contrast: F,
    /// `--pre-saturation`, `--pre-contrast` (K4).
    pub pre: Adjust,
    /// `--palette` (K1).
    pub palette: PaletteChoice,
}

impl Default for Settings {
    /// The CLI's defaults.
    fn default() -> Self {
        Self {
            method: Method::Pia,
            size: 128,
            colors: 16,
            oe_quantise: false,
            importance: ImportanceSource::Auto,
            // K2: the GUI defaults PixelOE to no dither; the CLI keeps upstream's ordered dither.
            oe: Options {
                dither: Dither::None,
                ..pixeloe_defaults()
            },
            pia_saturation: 1.1,
            oe_saturation: 1.0,
            contrast: 1.0,
            pre: Adjust::default(),
            palette: PaletteChoice::Native,
        }
    }
}

pub struct RunOutput {
    pub image: RgbImage,
    /// One line for the status bar.
    pub summary: String,
    pub warning: Option<String>,
}

/// Deserialises `oe` by laying the stored fields over the GUI's defaults (no dither, no
/// post-upscale, no post-match), so a partial object can't fall back to upstream's.
fn oe_over_gui_defaults<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Options, D::Error> {
    use serde::de::Error;
    use serde::Deserialize;
    let given = serde_json::Value::deserialize(d)?;
    let mut merged = serde_json::to_value(Settings::default().oe).map_err(D::Error::custom)?;
    match (merged.as_object_mut(), given) {
        (Some(base), serde_json::Value::Object(fields)) => base.extend(fields),
        (_, other) => {
            return Err(D::Error::custom(format!(
                "oe: expected an object, got {other}"
            )));
        }
    }
    serde_json::from_value(merged).map_err(D::Error::custom)
}

impl Settings {
    /// `--palette decimate-oklab|decimate-rgb` (K5); `None` = the method's own palette.
    fn decimate(&self) -> Option<Distance> {
        match self.palette {
            PaletteChoice::Native => None,
            PaletteChoice::DecimateOklab => Some(Distance::Oklab),
            PaletteChoice::DecimateRgb => Some(Distance::Rgb),
        }
    }

    /// The K4 adjustments for this method (`--pre-*`, `--contrast`, PixelOE `--saturation`).
    fn color_adjust(&self) -> ColorAdjust {
        ColorAdjust {
            pre: self.pre,
            post: Adjust {
                saturation: if self.method == Method::Pixeloe {
                    self.oe_saturation
                } else {
                    1.0
                },
                contrast: self.contrast,
            },
        }
    }

    /// Same as `pixplusplus <input> -s size -k colors [--importance ...] [--method pixeloe ...]`.
    pub fn run(&self, img: RgbImage) -> Result<RunOutput, String> {
        let t = Instant::now();
        let (iw, ih) = img.dimensions();
        let (ow, oh) = output_size(iw, ih, self.size, None, None);
        if self.method == Method::Pixeloe {
            let mut o = self.oe.clone();
            o.quant = self.oe_quantise.then_some(self.colors);
            let r = pixeloe_image(
                img,
                ow,
                oh,
                self.size,
                INPUT_FACTOR,
                o,
                &self.color_adjust(),
                self.decimate(),
            );
            let summary = format!(
                "PixelOE  {}x{} → {}x{}  pixel size {}  {} colours  {:.2} s",
                r.input_w,
                r.input_h,
                r.image.width(),
                r.image.height(),
                r.options.pixel_size,
                r.palette.len(),
                t.elapsed().as_secs_f64()
            );
            return Ok(RunOutput {
                image: r.image,
                summary,
                warning: None,
            });
        }
        let importance = match (self.method, &self.importance) {
            (Method::PiaImportance, ImportanceSource::Auto) => Importance::Auto,
            (Method::PiaImportance, ImportanceSource::File(p)) => Importance::Map(
                image::open(p)
                    .map_err(|e| format!("importance map {}: {e}", p.display()))?
                    .to_luma8(),
            ),
            _ => Importance::None,
        };
        let mut params = Params::new(ow, oh, self.colors);
        params.saturation = self.pia_saturation;
        let r = pia_image(
            img,
            params,
            INPUT_FACTOR,
            &importance,
            &self.color_adjust(),
            self.decimate(),
        );
        let s = &r.output.stats;
        let summary = format!(
            "{}  {}x{} → {}x{}  {} colours  {} iterations  {:.2} s",
            self.method.label(),
            r.input_w,
            r.input_h,
            r.output.width,
            r.output.height,
            s.palette_size,
            s.iterations,
            t.elapsed().as_secs_f64()
        );
        Ok(RunOutput {
            image: r.image,
            summary,
            warning: r.warning,
        })
    }
}

#[cfg(test)]
mod tests {
    //! The references in `tests/fixtures/gui/` are CLI outputs for `saliency_src.png`:
    //! `pixplusplus tests/fixtures/saliency_src.png -o <ref> <flags>` with the flags below.
    use super::*;

    fn check(s: Settings, reference: &str) {
        let src = image::open("tests/fixtures/saliency_src.png")
            .unwrap()
            .to_rgb8();
        let got = s.run(src).unwrap().image;
        let want = image::open(format!("tests/fixtures/gui/{reference}"))
            .unwrap()
            .to_rgb8();
        assert_eq!(got.dimensions(), want.dimensions());
        assert!(got == want, "{reference}: GUI output differs from the CLI");
    }

    #[test]
    fn pia_matches_cli() {
        // -s 64 -k 8
        let s = Settings {
            size: 64,
            colors: 8,
            ..Settings::default()
        };
        check(s, "pia_s64_k8.png");
    }

    #[test]
    fn pia_importance_auto_matches_cli() {
        // -s 48 -k 8 --importance auto
        let s = Settings {
            method: Method::PiaImportance,
            size: 48,
            colors: 8,
            ..Settings::default()
        };
        check(s, "pia_auto_s48_k8.png");
    }

    #[test]
    fn pixeloe_matches_cli() {
        // -s 48 -k 8 --method pixeloe (the CLI dithers by default; the GUI does not, K2)
        let mut s = Settings {
            method: Method::Pixeloe,
            size: 48,
            colors: 8,
            oe_quantise: true,
            ..Settings::default()
        };
        s.oe.dither = Dither::Ordered;
        check(s, "pixeloe_s48_k8.png");
    }

    #[test]
    fn pia_colour_controls_match_cli() {
        // -s 48 -k 8 --saturation 1.4 --contrast 1.2 --pre-saturation 1.3 --pre-contrast 0.9
        let s = Settings {
            size: 48,
            colors: 8,
            pia_saturation: 1.4,
            contrast: 1.2,
            pre: Adjust {
                saturation: 1.3,
                contrast: 0.9,
            },
            ..Settings::default()
        };
        check(s, "pia_s48_k8_colour.png");
    }

    #[test]
    fn pixeloe_colour_controls_match_cli() {
        // -s 48 -k 8 --method pixeloe --oe-dither none --saturation 1.5 --contrast 1.2
        //   --pre-saturation 1.3 --pre-contrast 1.1
        let s = Settings {
            method: Method::Pixeloe,
            size: 48,
            colors: 8,
            oe_quantise: true,
            oe_saturation: 1.5,
            contrast: 1.2,
            pre: Adjust {
                saturation: 1.3,
                contrast: 1.1,
            },
            ..Settings::default()
        };
        check(s, "pixeloe_s48_k8_colour.png");
    }

    #[test]
    fn pia_decimate_matches_cli() {
        // -s 48 -k 8 --palette decimate-rgb, and decimate-oklab
        for (palette, file) in [
            (PaletteChoice::DecimateRgb, "pia_s48_k8_decimate.png"),
            (
                PaletteChoice::DecimateOklab,
                "pia_s48_k8_decimate_oklab.png",
            ),
        ] {
            let s = Settings {
                size: 48,
                colors: 8,
                palette,
                ..Settings::default()
            };
            check(s, file);
        }
    }

    #[test]
    fn pixeloe_decimate_matches_cli() {
        // -s 48 -k 8 --method pixeloe --palette decimate-rgb, and decimate-oklab
        for (palette, file) in [
            (PaletteChoice::DecimateRgb, "pixeloe_s48_k8_decimate.png"),
            (
                PaletteChoice::DecimateOklab,
                "pixeloe_s48_k8_decimate_oklab.png",
            ),
        ] {
            let s = Settings {
                method: Method::Pixeloe,
                size: 48,
                colors: 8,
                oe_quantise: true,
                palette,
                ..Settings::default()
            };
            check(s, file);
        }
    }

    #[test]
    fn pixeloe_unquantised_matches_cli() {
        // -s 48 --method pixeloe --oe-mode k-centroid --oe-dither none
        let mut s = Settings {
            method: Method::Pixeloe,
            size: 48,
            ..Settings::default()
        };
        s.oe.mode = pixplusplus::pixeloe::pipeline::Mode::KCentroid;
        check(s, "pixeloe_kc_s48.png");
    }
}
