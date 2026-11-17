//! PixelOE in Rust (G1): a port of the torch backend of PixelOE
//! (KohakuBlueleaf/PixelOE, Apache-2.0, vendored at `refs/PixelOE` commit 7ce444b).
//!
//! Images are planar `f32` (channel, row, column), like the `[B, C, H, W]` tensors upstream
//! with B = 1, so each function maps one-to-one onto its Python original. Every stage is
//! checked against reference dumps of the Python code (`bench/pixeloe_ref.py`,
//! `tests/fixtures/pixeloe/`).

pub mod downscale;
pub mod lab;
pub mod morph;
pub mod outline;
pub mod sharpen;

/// A planar float image: `data[(c * h + y) * w + x]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Img {
    pub c: usize,
    pub h: usize,
    pub w: usize,
    pub data: Vec<f32>,
}

impl Img {
    pub fn new(c: usize, h: usize, w: usize) -> Self {
        Self {
            c,
            h,
            w,
            data: vec![0.0; c * h * w],
        }
    }

    pub fn from_vec(c: usize, h: usize, w: usize, data: Vec<f32>) -> Self {
        assert_eq!(data.len(), c * h * w);
        Self { c, h, w, data }
    }

    #[inline]
    pub fn at(&self, c: usize, y: usize, x: usize) -> f32 {
        self.data[(c * self.h + y) * self.w + x]
    }

    #[inline]
    pub fn set(&mut self, c: usize, y: usize, x: usize, v: f32) {
        self.data[(c * self.h + y) * self.w + x] = v;
    }

    /// One channel as a slice.
    pub fn plane(&self, c: usize) -> &[f32] {
        &self.data[c * self.h * self.w..(c + 1) * self.h * self.w]
    }

    /// Channel `c` as a single-channel image.
    pub fn channel(&self, c: usize) -> Img {
        Img::from_vec(1, self.h, self.w, self.plane(c).to_vec())
    }

    pub fn map(&self, f: impl Fn(f32) -> f32) -> Img {
        Img::from_vec(
            self.c,
            self.h,
            self.w,
            self.data.iter().map(|&v| f(v)).collect(),
        )
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Loader for the Python reference dumps (raw little-endian f32 + manifest of shapes).
    use super::Img;
    use std::path::PathBuf;

    fn dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pixeloe")
    }

    pub fn shape(name: &str) -> Vec<usize> {
        let m: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir().join("manifest.json")).unwrap())
                .unwrap();
        m["arrays"][name]
            .as_array()
            .unwrap_or_else(|| panic!("no fixture {name}"))
            .iter()
            .map(|v| v.as_u64().unwrap() as usize)
            .collect()
    }

    pub fn raw(name: &str) -> Vec<f32> {
        std::fs::read(dir().join(format!("{name}.f32")))
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect()
    }

    /// A `[1, C, H, W]` or `[H, W]` array as an image.
    pub fn img(name: &str) -> Img {
        let s = shape(name);
        let (c, h, w) = match s.as_slice() {
            [1, c, h, w] => (*c, *h, *w),
            [h, w] => (1, *h, *w),
            _ => panic!("unexpected shape {s:?} for {name}"),
        };
        Img::from_vec(c, h, w, raw(name))
    }

    /// Max absolute difference, after checking shapes match.
    pub fn max_diff(got: &Img, want: &Img) -> f32 {
        assert_eq!((got.c, got.h, got.w), (want.c, want.h, want.w), "shape");
        got.data
            .iter()
            .zip(&want.data)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max)
    }

    pub fn assert_close(name: &str, got: &Img, tol: f32) {
        let want = img(name);
        let d = max_diff(got, &want);
        eprintln!("{name}: max |diff| {d:.2e}");
        assert!(d <= tol, "{name}: max |diff| {d} > {tol}");
    }
}
