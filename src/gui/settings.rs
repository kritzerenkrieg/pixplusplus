//! GUI settings (J3) and the run that turns them into an image through the same glue as the CLI
//! (`pixplusplus::job`, J2). Settings the GUI doesn't show keep the CLI defaults.

use image::RgbImage;
use pixplusplus::job::{output_size, pia_image, pixeloe_defaults, pixeloe_image, Importance};
use pixplusplus::pixeloe::pipeline::Options;
use pixplusplus::Params;
use std::path::PathBuf;
use std::time::Instant;

/// `--input-factor` default (B3, G7); not shown in the GUI (J3).
pub const INPUT_FACTOR: f64 = 4.0;

/// The three methods (E8).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

/// Where the importance map comes from (C1, E7).
#[derive(Clone, PartialEq, Debug)]
pub enum ImportanceSource {
    /// Spectral-residual saliency, `--importance auto` (E7).
    Auto,
    /// A grayscale image, `--importance FILE`.
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub method: Method,
    /// `-s`: output long side.
    pub size: usize,
    /// `-k`: palette size, a hard limit (B4, G10).
    pub colors: usize,
    /// PixelOE quantises only when `-k` is given (G8).
    pub oe_quantise: bool,
    pub importance: ImportanceSource,
    /// `--oe-*` options (G9); `quant` and `pixel_size` are set at run time.
    pub oe: Options,
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
            oe: pixeloe_defaults(),
        }
    }
}

pub struct RunOutput {
    pub image: RgbImage,
    /// One line for the status bar.
    pub summary: String,
    pub warning: Option<String>,
}

impl Settings {
    /// Same as `pixplusplus <input> -s size -k colors [--importance ...] [--method pixeloe ...]`.
    pub fn run(&self, img: RgbImage) -> Result<RunOutput, String> {
        let t = Instant::now();
        let (iw, ih) = img.dimensions();
        let (ow, oh) = output_size(iw, ih, self.size, None, None);
        if self.method == Method::Pixeloe {
            let mut o = self.oe.clone();
            o.quant = self.oe_quantise.then_some(self.colors);
            let r = pixeloe_image(img, ow, oh, self.size, INPUT_FACTOR, o);
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
        let r = pia_image(
            img,
            Params::new(ow, oh, self.colors),
            INPUT_FACTOR,
            &importance,
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
        // -s 48 -k 8 --method pixeloe
        let s = Settings {
            method: Method::Pixeloe,
            size: 48,
            colors: 8,
            oe_quantise: true,
            ..Settings::default()
        };
        check(s, "pixeloe_s48_k8.png");
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
        s.oe.dither = pixplusplus::pixeloe::color::Dither::None;
        check(s, "pixeloe_kc_s48.png");
    }
}
