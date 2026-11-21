//! Balanced palette (M1): main colours first, distinct accents by priority, then shades.
//!
//! Plain decimation (K1) spends slots on whatever is far apart, and lightness counts as much as
//! hue there, so the shading steps of one large area each keep a slot while a small accent, or a
//! second hue of similar brightness, is merged away. Here slots are shared in three steps:
//!
//! 1. **Families.** Colours are grouped into hue families by quick shift on a k-nearest-neighbour
//!    density (Vedaldi & Soatto 2008): each colour links to its nearest denser colour within
//!    [`TAU`], and each root is a family. Distances use the hue-emphasis metric (lightness scaled
//!    by [`W_L`]), so a light-to-dark ramp of one hue is one family and neutrals form their own.
//! 2. **Priority.** A family's first slot is worth what leaving it out costs: its weight times
//!    the squared distance to the nearest colour picked so far, `w_f · d²` (the heaviest family
//!    goes first). Each further slot in a family is worth the cost of the next split in its
//!    Ward tree (Ward 1963), the squared error that split removes. So a big area of a distinct hue ranks
//!    high, a small colour ranks high when it's far from everything (an accent), and a shade
//!    that's close to its neighbours ranks low. There's no quota: all gains form one priority
//!    list, re-ranked after each pick, and `k` picks are made; a family's splits wait for its
//!    first slot.
//! 3. **Representatives.** Each final cluster keeps its heaviest exact colour, as K1 does, so the
//!    palette is made of input colours and isn't greyed by averaging.
//!
//! Costs use the same hue-emphasis metric as the families, so a lightness step counts for less
//! than an equal step in hue or chroma. Pixels map to the nearest palette colour in that metric
//! too (`Distance::Balanced` in [`super::coords`]).

use super::{coords, Distance};
use std::cmp::Ordering;

/// Weight of Oklab lightness against a and b (M1). Below 1, hue and chroma count more than
/// lightness, so shades of one hue sit closer together than different hues. Pix++ choice,
/// tuned on the 2026-11-21 showcase.
pub const W_L: crate::F = 0.5;
/// Neighbours in each colour's density estimate (the "k nearest" of M1). Pix++ choice.
const KNN: usize = 8;
/// Linking radius for families, in the hue-emphasis metric (Oklab ×100 units): colours further
/// apart than this never join one family directly. Also the density kernel's width. Pix++
/// choice, tuned on the 2026-11-21 showcase.
const TAU: f64 = 8.0;

type P = [f64; 3];

fn d2(a: &P, b: &P) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// Ward cost of merging clusters of weight `wa`, `wb` with centroids `ca`, `cb`.
fn ward(wa: f64, ca: &P, wb: f64, cb: &P) -> f64 {
    wa * wb / (wa + wb) * d2(ca, cb)
}

/// Up to `k` colours from bucketed colours `cols` (colour, pixel count), heaviest first.
pub(super) fn palette(cols: &[([u8; 3], u64)], k: usize) -> Vec<[u8; 3]> {
    let pos: Vec<P> = cols
        .iter()
        .map(|&(c, _)| coords(c, Distance::Balanced).map(f64::from))
        .collect();
    let w: Vec<f64> = cols.iter().map(|&(_, n)| n as f64).collect();

    let fam = families(&pos, &w);
    let nf = fam.iter().max().map_or(0, |&f| f + 1);
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); nf];
    for (i, &f) in fam.iter().enumerate() {
        members[f].push(i);
    }
    let trees: Vec<Tree> = members.iter().map(|m| Tree::new(m, &pos, &w)).collect();
    // One priority list over every family's next gain, re-ranked after each pick: a family's
    // first slot is worth `w_f · d²` from its centroid to the nearest colour picked so far (what
    // leaving it out costs); a picked family's next split is worth its Ward cost. The heaviest
    // family goes first. A family's splits wait for its first slot.
    let mut cut: Vec<Vec<usize>> = vec![Vec::new(); nf];
    // Squared distance from each family's centroid to the nearest picked colour.
    let mut near = vec![f64::INFINITY; nf];
    let root = |f: usize| &trees[f].nodes[trees[f].root()];
    let mut taken = 0;
    while taken < k {
        let mut best: Option<Gain> = None;
        for f in 0..nf {
            let g = if cut[f].is_empty() {
                let value = if taken == 0 {
                    root(f).weight
                } else {
                    root(f).weight * near[f]
                };
                Some(Gain {
                    value,
                    family: f,
                    node: None,
                })
            } else {
                next_split(&trees[f], &cut[f]).map(|(node, value)| Gain {
                    value,
                    family: f,
                    node: Some(node),
                })
            };
            if let Some(g) = g {
                if best.as_ref().is_none_or(|b| g > *b) {
                    best = Some(g);
                }
            }
        }
        let Some(g) = best else { break };
        let t = &trees[g.family];
        let c = &mut cut[g.family];
        let added = match g.node {
            None => {
                c.push(t.root());
                vec![t.root()]
            }
            Some(node) => {
                c.retain(|&x| x != node);
                let (a, b) = t.nodes[node].kids.expect("only internal nodes are split");
                c.extend([a, b]);
                vec![a, b]
            }
        };
        for x in added {
            let p = &pos[t.nodes[x].best];
            for (f, n) in near.iter_mut().enumerate() {
                *n = n.min(d2(&root(f).centroid, p));
            }
        }
        taken += 1;
    }

    let mut out: Vec<([u8; 3], f64)> = cut
        .iter()
        .enumerate()
        .flat_map(|(f, c)| c.iter().map(move |&x| (f, x)))
        .map(|(f, x)| {
            let nd = &trees[f].nodes[x];
            (cols[nd.best].0, nd.weight)
        })
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out.into_iter().map(|(c, _)| c).collect()
}

/// A priority-list entry: a family's first slot (`node` None) or the split of one of its nodes.
struct Gain {
    value: f64,
    family: usize,
    node: Option<usize>,
}

impl PartialEq for Gain {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Gain {}
impl PartialOrd for Gain {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Gain {
    /// Larger gain first; ties go to the lower family index, so the result is deterministic.
    fn cmp(&self, o: &Self) -> Ordering {
        self.value
            .total_cmp(&o.value)
            .then(o.family.cmp(&self.family))
    }
}

/// Family index of each colour: quick shift on a k-nearest-neighbour density.
fn families(pos: &[P], w: &[f64]) -> Vec<usize> {
    let n = pos.len();
    let kernel = |d: f64| (-d / (TAU * TAU)).exp();
    // Density: own weight plus the kernel-weighted weight of the KNN nearest colours.
    let density: Vec<f64> = (0..n)
        .map(|i| {
            let mut near: Vec<(f64, usize)> = (0..n)
                .filter(|&j| j != i)
                .map(|j| (d2(&pos[i], &pos[j]), j))
                .collect();
            let m = KNN.min(near.len());
            if m < near.len() {
                near.select_nth_unstable_by(m, |a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            }
            w[i] + near[..m]
                .iter()
                .map(|&(d, j)| w[j] * kernel(d))
                .sum::<f64>()
        })
        .collect();
    // Denser, with ties broken by index, so the links form a forest.
    let denser =
        |j: usize, i: usize| density[j] > density[i] || (density[j] == density[i] && j < i);
    let parent: Vec<usize> = (0..n)
        .map(|i| {
            let mut best = (i, TAU * TAU);
            for j in 0..n {
                if j != i && denser(j, i) {
                    let d = d2(&pos[i], &pos[j]);
                    if d < best.1 {
                        best = (j, d);
                    }
                }
            }
            best.0
        })
        .collect();
    let mut fam = vec![usize::MAX; n];
    let mut next = 0;
    for i in 0..n {
        let mut r = i;
        while parent[r] != r {
            r = parent[r];
        }
        if fam[r] == usize::MAX {
            fam[r] = next;
            next += 1;
        }
        fam[i] = fam[r];
    }
    fam
}

/// A family's next split: its costliest internal node in the cut, and that node's Ward cost.
fn next_split(t: &Tree, cut: &[usize]) -> Option<(usize, f64)> {
    cut.iter()
        .filter(|&&x| t.nodes[x].kids.is_some())
        .map(|&x| (x, t.nodes[x].cost))
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
}

struct Node {
    /// Ward cost of the merge that made this node (0 for a leaf).
    cost: f64,
    kids: Option<(usize, usize)>,
    weight: f64,
    centroid: P,
    /// Index into the bucketed colours of this node's heaviest colour.
    best: usize,
}

/// A family's Ward tree. Nodes 0..m are its colours; the last node is the root.
struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    /// Ward clustering by nearest-neighbour chains, O(m²) (Murtagh 1983).
    fn new(members: &[usize], pos: &[P], w: &[f64]) -> Tree {
        let mut nodes: Vec<Node> = members
            .iter()
            .map(|&i| Node {
                cost: 0.0,
                kids: None,
                weight: w[i],
                centroid: pos[i],
                best: i,
            })
            .collect();
        let m = members.len();
        // Live clusters by slot; slot s holds node `id[s]`.
        let mut id: Vec<usize> = (0..m).collect();
        let mut live: Vec<usize> = (0..m).collect();
        let cost = |nodes: &[Node], a: usize, b: usize| {
            let (x, y) = (&nodes[a], &nodes[b]);
            ward(x.weight, &x.centroid, y.weight, &y.centroid)
        };
        let mut chain: Vec<usize> = Vec::new();
        while live.len() > 1 {
            if chain.is_empty() {
                chain.push(live[0]);
            }
            let a = *chain.last().unwrap();
            let prev = chain.len().checked_sub(2).map(|i| chain[i]);
            // Nearest live slot; a tie keeps the chain's previous slot, so the chain ends.
            let mut best = prev.map_or((usize::MAX, f64::INFINITY), |p| {
                (p, cost(&nodes, id[a], id[p]))
            });
            for &s in &live {
                if s != a {
                    let c = cost(&nodes, id[a], id[s]);
                    if c < best.1 {
                        best = (s, c);
                    }
                }
            }
            if Some(best.0) == prev {
                chain.truncate(chain.len() - 2);
                let (x, y) = (id[a], id[best.0]);
                let (nx, ny) = (&nodes[x], &nodes[y]);
                let wt = nx.weight + ny.weight;
                let c = [0, 1, 2]
                    .map(|k| (nx.centroid[k] * nx.weight + ny.centroid[k] * ny.weight) / wt);
                let heavier =
                    if w[ny.best] > w[nx.best] || (w[ny.best] == w[nx.best] && ny.best < nx.best) {
                        ny.best
                    } else {
                        nx.best
                    };
                nodes.push(Node {
                    cost: best.1,
                    kids: Some((x, y)),
                    weight: wt,
                    centroid: c,
                    best: heavier,
                });
                id[a] = nodes.len() - 1;
                live.retain(|&s| s != best.0);
            } else {
                chain.push(best.0);
            }
        }
        Tree { nodes }
    }

    fn root(&self) -> usize {
        self.nodes.len() - 1
    }
}
