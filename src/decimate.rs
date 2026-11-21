//! Palette by colour decimation (K1, K5) and the balanced palette (K6, [`balanced`]).
//!
//! Repeatedly merge the two closest colours, keeping the heavier one (more pixels) unchanged,
//! until `k` remain; then map every pixel to its nearest remaining colour, with no dither.
//! Because a merge keeps a real colour instead of averaging two, vivid colours that cover few
//! pixels are not pulled towards grey the way k-means and PIA's annealed palette pull them.
//!
//! Colours are first grouped into buckets of nearby colours (top bits per channel), which pools
//! the weight of noisy near-duplicates (a grainy gradient is thousands of colours with a few
//! pixels each). Each bucket is represented by its most common exact colour, so every palette
//! colour is one of the input's colours. If the buckets number `k` or fewer, the merge runs on
//! the exact colours when there are at most `MAX_BUCKETS` of them, else on finer buckets.

mod balanced;

use crate::color::srgb_to_oklab;
use crate::F;
use image::RgbImage;
use std::collections::HashMap;

/// How colour distance is measured when merging and mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Distance {
    /// Euclidean distance on 8-bit sRGB (the user's proposal, K1).
    Rgb,
    /// Euclidean distance in Oklab.
    Oklab,
    /// Balanced palette (K6): hue families first, distinct accents by priority, then shades.
    /// Pixels map by the hue-emphasis distance ([`balanced::W_L`]).
    Balanced,
}

/// PIA palette size to solve at before decimating to `k` (K1): 4k, at least 64, at most 256,
/// never below k. The spike solved at 64 for k = 8.
pub fn pia_solve_k(k: usize) -> usize {
    (4 * k).clamp(64, 256).max(k)
}

/// Most buckets the merge starts from; with more, buckets get coarser. The merge is about
/// quadratic in this, so it bounds the time on noisy inputs.
const MAX_BUCKETS: usize = 4096;
/// Finest bucket: 5 bits per channel (32 levels).
const FINE_BITS: u32 = 5;

fn coords(c: [u8; 3], d: Distance) -> [F; 3] {
    match d {
        Distance::Rgb => c.map(|v| v as F),
        Distance::Oklab => oklab100(c),
        Distance::Balanced => {
            let [l, a, b] = oklab100(c);
            [balanced::W_L * l, a, b]
        }
    }
}

/// Oklab ×100 (L 0..100, a and b roughly ±40) of an 8-bit sRGB colour.
fn oklab100(c: [u8; 3]) -> [F; 3] {
    srgb_to_oklab(c.map(|v| v as F / 255.0)).map(|v| v * 100.0)
}

fn d2(a: &[F; 3], b: &[F; 3]) -> F {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// Pixel counts per exact colour, in first-seen order (deterministic).
fn histogram(img: &RgbImage) -> Vec<([u8; 3], u64)> {
    let mut idx: HashMap<[u8; 3], usize> = HashMap::new();
    let mut hist: Vec<([u8; 3], u64)> = Vec::new();
    for p in img.pixels() {
        let i = *idx.entry(p.0).or_insert_with(|| {
            hist.push((p.0, 0));
            hist.len() - 1
        });
        hist[i].1 += 1;
    }
    hist
}

/// Groups colours into buckets of `bits` per channel. Each bucket: its most common exact colour
/// (first seen wins ties) and its total weight.
fn buckets(hist: &[([u8; 3], u64)], bits: u32) -> Vec<([u8; 3], u64)> {
    let shift = 8 - bits;
    let mut idx: HashMap<[u8; 3], usize> = HashMap::new();
    // (representative, its count, total weight)
    let mut out: Vec<([u8; 3], u64, u64)> = Vec::new();
    for &(c, n) in hist {
        let key = c.map(|v| v >> shift);
        let i = *idx.entry(key).or_insert_with(|| {
            out.push((c, 0, 0));
            out.len() - 1
        });
        let b = &mut out[i];
        if n > b.1 {
            (b.0, b.1) = (c, n);
        }
        b.2 += n;
    }
    out.into_iter().map(|(c, _, w)| (c, w)).collect()
}

/// Up to `k` palette colours for `img`, each one of `img`'s own colours, heaviest first.
pub fn palette(img: &RgbImage, k: usize, dist: Distance) -> Vec<[u8; 3]> {
    assert!(k >= 1, "palette size must be at least 1");
    let hist = histogram(img);
    if hist.len() <= k {
        let mut h = hist;
        h.sort_by_key(|c| std::cmp::Reverse(c.1));
        return h.into_iter().map(|(c, _)| c).collect();
    }
    let mut bits = FINE_BITS;
    let mut cols = buckets(&hist, bits);
    while cols.len() > MAX_BUCKETS && bits > 1 {
        bits -= 1;
        cols = buckets(&hist, bits);
    }
    if cols.len() <= k {
        // Bucketing alone got below k. Use the exact colours if the merge can afford them;
        // otherwise refine the buckets while they stay within MAX_BUCKETS. Each extra bit splits
        // a bucket into at most 8, so this gets above k for any k <= MAX_BUCKETS / 8.
        if hist.len() <= MAX_BUCKETS {
            cols = hist;
        } else {
            while cols.len() <= k && bits < 7 {
                let finer = buckets(&hist, bits + 1);
                if finer.len() > MAX_BUCKETS {
                    break;
                }
                bits += 1;
                cols = finer;
            }
        }
    }

    if dist == Distance::Balanced {
        return balanced::palette(&cols, k);
    }

    let n = cols.len();
    let pos: Vec<[F; 3]> = cols.iter().map(|&(c, _)| coords(c, dist)).collect();
    let mut weight: Vec<u64> = cols.iter().map(|&(_, w)| w).collect();
    let mut alive = vec![true; n];
    // Nearest live neighbour of each live colour.
    let nearest = |i: usize, alive: &[bool]| -> (usize, F) {
        let mut best = (usize::MAX, F::INFINITY);
        for j in 0..n {
            if j != i && alive[j] {
                let d = d2(&pos[i], &pos[j]);
                if d < best.1 {
                    best = (j, d);
                }
            }
        }
        best
    };
    let mut nn: Vec<(usize, F)> = (0..n).map(|i| nearest(i, &alive)).collect();

    let mut left = n;
    while left > k {
        // Closest pair; ties go to the lowest index, so the result is deterministic.
        let (mut a, mut best) = (usize::MAX, F::INFINITY);
        for i in 0..n {
            if alive[i] && nn[i].1 < best {
                (a, best) = (i, nn[i].1);
            }
        }
        let b = nn[a].0;
        // The heavier colour survives unchanged (ties: the lower index).
        let (keep, gone) = if weight[b] > weight[a] || (weight[b] == weight[a] && b < a) {
            (b, a)
        } else {
            (a, b)
        };
        weight[keep] += weight[gone];
        alive[gone] = false;
        left -= 1;
        // Survivors don't move, so only colours whose nearest was `gone` need a new one.
        for i in 0..n {
            if alive[i] && nn[i].0 == gone {
                nn[i] = nearest(i, &alive);
            }
        }
    }

    let mut out: Vec<([u8; 3], u64)> = (0..n)
        .filter(|&i| alive[i])
        .map(|i| (cols[i].0, weight[i]))
        .collect();
    out.sort_by_key(|c| std::cmp::Reverse(c.1));
    out.into_iter().map(|(c, _)| c).collect()
}

/// For each colour in `img`, its nearest colour in `pal` (no dither).
pub fn mapping(img: &RgbImage, pal: &[[u8; 3]], dist: Distance) -> HashMap<[u8; 3], [u8; 3]> {
    let pp: Vec<[F; 3]> = pal.iter().map(|&c| coords(c, dist)).collect();
    let mut map = HashMap::new();
    for p in img.pixels() {
        map.entry(p.0).or_insert_with(|| {
            let c = coords(p.0, dist);
            let mut best = (0, F::INFINITY);
            for (j, q) in pp.iter().enumerate() {
                let d = d2(&c, q);
                if d < best.1 {
                    best = (j, d);
                }
            }
            pal[best.0]
        });
    }
    map
}

/// Decimates `img` to at most `k` colours in place; returns the palette (heaviest first).
pub fn decimate(img: &mut RgbImage, k: usize, dist: Distance) -> Vec<[u8; 3]> {
    let pal = palette(img, k, dist);
    let map = mapping(img, &pal, dist);
    for p in img.pixels_mut() {
        p.0 = map[&p.0];
    }
    pal
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn noisy() -> RgbImage {
        // Smooth gradients plus a small, vivid red patch that averaging would grey out.
        RgbImage::from_fn(96, 64, |x, y| {
            if (40..46).contains(&x) && (20..26).contains(&y) {
                image::Rgb([230, 20, 30])
            } else {
                image::Rgb([
                    (60 + x) as u8,
                    (90 + y * 2) as u8,
                    (120 + ((x * 7 + y * 13) % 23)) as u8,
                ])
            }
        })
    }

    fn colours(img: &RgbImage) -> HashSet<[u8; 3]> {
        img.pixels().map(|p| p.0).collect()
    }

    #[test]
    fn at_most_k_colours_all_from_the_input() {
        for dist in [Distance::Rgb, Distance::Oklab, Distance::Balanced] {
            for k in [1, 2, 8, 16] {
                let src = noisy();
                let before = colours(&src);
                let mut img = src.clone();
                let pal = decimate(&mut img, k, dist);
                let after = colours(&img);
                assert!(after.len() <= k, "{dist:?} k={k}: {} colours", after.len());
                assert!(pal.len() <= k);
                assert!(after.iter().all(|c| before.contains(c)), "{dist:?} k={k}");
                assert!(after.iter().all(|c| pal.contains(c)));
            }
        }
    }

    #[test]
    fn keeps_a_small_vivid_colour() {
        let mut img = noisy();
        let pal = decimate(&mut img, 8, Distance::Rgb);
        assert!(pal.contains(&[230, 20, 30]), "{pal:?}");
    }

    #[test]
    fn many_colours_in_few_buckets_stay_bounded() {
        // 65536 exact colours in 128 five-bit buckets: the merge must not get all 65536.
        let img = RgbImage::from_fn(256, 256, |x, y| {
            let i = y * 256 + x;
            image::Rgb([(i % 64) as u8, (i / 64 % 64) as u8, (i / 4096) as u8])
        });
        for dist in [Distance::Rgb, Distance::Balanced] {
            let t = std::time::Instant::now();
            let pal = palette(&img, 256, dist);
            assert_eq!(pal.len(), 256, "{dist:?}");
            assert!(
                t.elapsed().as_secs() < 30,
                "{dist:?} took {:?}",
                t.elapsed()
            );
        }
    }

    #[test]
    fn few_colours_are_kept_as_is() {
        let src = RgbImage::from_fn(4, 4, |x, _| image::Rgb([x as u8 * 60, 0, 0]));
        let mut img = src.clone();
        decimate(&mut img, 8, Distance::Rgb);
        assert_eq!(img, src);
    }

    #[test]
    fn deterministic() {
        for dist in [Distance::Oklab, Distance::Balanced] {
            let (mut a, mut b) = (noisy(), noisy());
            assert_eq!(decimate(&mut a, 6, dist), decimate(&mut b, 6, dist));
            assert_eq!(a, b);
        }
    }

    /// Two muted hues 11 apart in a/b, each a ramp of 4 shades 12 apart in L (400 px each), and
    /// a small vivid accent (80 px). Returns the image and each ramp's and the accent's colours.
    fn ramps_and_accent() -> (RgbImage, [HashSet<[u8; 3]>; 3]) {
        let srgb = |l: F, a: F, b: F| {
            crate::color::oklab_to_srgb([l / 100.0, a / 100.0, b / 100.0])
                .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        };
        let ramp = |a: F, b: F| [40.0, 52.0, 64.0, 76.0].map(|l| srgb(l, a, b));
        let (ra, rb, acc) = (ramp(2.0, 6.0), ramp(-7.0, 0.0), srgb(55.0, 25.0, -15.0));
        let mut px: Vec<[u8; 3]> = Vec::new();
        for c in ra.iter().chain(&rb) {
            px.extend(std::iter::repeat_n(*c, 400));
        }
        px.extend(std::iter::repeat_n(acc, 80));
        let img = RgbImage::from_fn(82, 40, |x, y| image::Rgb(px[(y * 82 + x) as usize]));
        let set = |c: &[[u8; 3]]| c.iter().copied().collect::<HashSet<_>>();
        (img, [set(&ra), set(&rb), set(&[acc])])
    }

    /// Which groups (ramp A, ramp B, accent) the palette has a colour from.
    fn groups(pal: &[[u8; 3]], g: &[HashSet<[u8; 3]>; 3]) -> [bool; 3] {
        g.each_ref().map(|s| pal.iter().any(|c| s.contains(c)))
    }

    #[test]
    fn balanced_keeps_both_hues_before_any_shade() {
        let (img, g) = ramps_and_accent();
        // Plain Oklab merges the two hues first (11 apart, closer than a shade step).
        assert_eq!(
            groups(&palette(&img, 3, Distance::Oklab), &g),
            [true, false, true]
        );
        assert_eq!(
            groups(&palette(&img, 2, Distance::Balanced), &g),
            [true, true, false]
        );
    }

    #[test]
    fn balanced_accent_ranks_above_shades() {
        let (img, g) = ramps_and_accent();
        let pal = palette(&img, 3, Distance::Balanced);
        assert_eq!(groups(&pal, &g), [true, true, true], "{pal:?}");
        // The fourth slot goes to a shade.
        let pal = palette(&img, 4, Distance::Balanced);
        assert_eq!(pal.len(), 4);
        assert_eq!(groups(&pal, &g), [true, true, true]);
    }
}
