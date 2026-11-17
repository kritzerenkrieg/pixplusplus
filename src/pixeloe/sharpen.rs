//! PixelOE sharpening (`pixeloe/torch/sharpen/{unsharp,laplacian}.py`): 3x3 depthwise
//! convolutions with zero padding, as `F.conv2d(..., padding=1, groups=C)`.

use super::Img;

/// Depthwise 3x3 convolution (cross-correlation, like torch) with zero padding.
fn conv3x3(img: &Img, k: &[[f32; 3]; 3]) -> Img {
    let mut out = Img::new(img.c, img.h, img.w);
    for c in 0..img.c {
        for y in 0..img.h {
            for x in 0..img.w {
                let mut s = 0.0f32;
                for (i, row) in k.iter().enumerate() {
                    let yy = y as isize + i as isize - 1;
                    if yy < 0 || yy >= img.h as isize {
                        continue;
                    }
                    for (j, &kv) in row.iter().enumerate() {
                        let xx = x as isize + j as isize - 1;
                        if xx < 0 || xx >= img.w as isize {
                            continue;
                        }
                        s += img.at(c, yy as usize, xx as usize) * kv;
                    }
                }
                out.set(c, y, x, s);
            }
        }
    }
    out
}

/// PixelOE `unsharp_mask(x, kernel_size=3, sigma=1.0, amount, threshold=0.1)`, the form
/// `pixelize` calls: details smaller than the threshold are left alone.
pub fn unsharp(img: &Img, amount: f32) -> Img {
    const SIGMA: f32 = 1.0;
    const THRESHOLD: f32 = 0.1;
    let g: [f32; 3] = [-1.0f32, 0.0, 1.0].map(|x| (-(x * x) / (2.0 * SIGMA * SIGMA)).exp());
    let sum: f32 = g.iter().sum();
    let k1 = g.map(|v| v / sum);
    let k = [0, 1, 2].map(|i| [0, 1, 2].map(|j| k1[i] * k1[j]));
    let blurred = conv3x3(img, &k);
    Img::from_vec(
        img.c,
        img.h,
        img.w,
        img.data
            .iter()
            .zip(&blurred.data)
            .map(|(&x, &b)| {
                let m = x - b;
                let m = if m.abs() < THRESHOLD { 0.0 } else { m };
                (x + amount * m).clamp(0.0, 1.0)
            })
            .collect(),
    )
}

/// PixelOE `laplacian_sharpen(x, amount)`.
pub fn laplacian(img: &Img, amount: f32) -> Img {
    let k = [[0.0, -1.0, 0.0], [-1.0, 4.0, -1.0], [0.0, -1.0, 0.0]];
    let edges = conv3x3(img, &k);
    Img::from_vec(
        img.c,
        img.h,
        img.w,
        img.data
            .iter()
            .zip(&edges.data)
            .map(|(&x, &e)| (x + amount * e).clamp(0.0, 1.0))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixeloe::fixtures::{assert_close, img};
    use crate::pixeloe::outline::{outline_expansion, Mapping, Normalize};

    #[test]
    fn sharpen_matches() {
        let (expanded, _) =
            outline_expansion(&img("a_img"), 2, 2, 4, Mapping::Current, Normalize::Global);
        assert_close("a_unsharp", &unsharp(&expanded, 0.5), 1e-5);
        assert_close("a_laplacian", &laplacian(&expanded, 0.5), 1e-5);
    }
}
