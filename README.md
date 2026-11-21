# Pix++

Pix++ (`pixplusplus`) is a procedural pixelization program: concept art in, pixel art out.
It comes as a command-line tool and a desktop GUI, written in Rust.

Two methods:

- **Pixelated Image Abstraction (PIA)**, the CLI default (Gerstner et al., NPAR 2012 /
  C&G 2013). Superpixels are the output pixels; segmentation and palette are solved jointly
  with mass-constrained deterministic annealing in Oklab. Optional **importance map**: brighter
  areas get more palette mass. By default Pix++ computes one with spectral-residual saliency
  (`--importance auto`); `--importance none` gives plain PIA, and the CLI also takes your own
  grayscale map.
- **Pixelization on Outline Expansion (PixelOE)**: a port of [PixelOE](https://github.com/KohakuBlueleaf/PixelOE)'s torch backend
  (outline expansion, contrast-aware downscale, optional k-means palette and dithering).
  It matches upstream within 0.001 mean ΔE and is faster than torch on CPU.

## Build

Needs Rust stable ([rustup](https://rustup.rs)). The GUI on Linux also needs the GTK 3
development files for its native file dialogs (Debian/Ubuntu: `sudo apt install libgtk-3-dev`);
the CLI needs nothing else.

```sh
cargo build --release                  # CLI: target/release/pixplusplus
cargo build --release --features gui   # also the GUI: target/release/pixplusplus-gui
```

The default build has no GUI dependencies. The GUI uses egui/eframe with native file dialogs
and targets Linux and Windows (macOS is not supported for now).

## GUI

```sh
./target/release/pixplusplus-gui
```

Open an image, pick a method, set the size, colours and palette, adjust colour, press Run, save.

- **Methods:** *Pixelated Image Abstraction* (PIA) and *Pixelization on Outline Expansion*
  (PixelOE). Under PIA, the automatic importance map is on by default; untick it for plain PIA.
  PixelOE has no dithering by default in the GUI, and its detailed options are folded away.
- **Keys:** Ctrl+O open, Ctrl+Enter run, Ctrl+S save PNG. Hold Space on the result to peek at the
  source. A notice shows when the settings changed since the result on screen.
- Jobs run on a worker thread, so the window stays responsive.

Settings can be kept as named profiles (Save, Save as…, Load, Delete at the top of the panel).
Each profile is a JSON file in `~/.config/pixplusplus/profiles` on Linux (`$XDG_CONFIG_HOME`
if set) and `%APPDATA%\pixplusplus\profiles` on Windows.

## CLI

PIA:

```sh
./target/release/pixplusplus art.png -o out.png -s 256 -k 16 --preview 4
#   -s 256                 output size (longest side), -k 16 palette colours
#   --preview 4            also write a 4x nearest-neighbour upscale
#   --importance auto      (default) spectral-residual saliency computed from the input
#   --importance none      plain PIA, no importance map
#   --importance map.png   your own grayscale map; brighter areas get more palette mass
#   --stats run.json       timings / iterations / peak RSS
#   --pyxeled-compat --space cielab   parity mode against Pyxeled
```

PixelOE:

```sh
./target/release/pixplusplus art.png -o out.png -s 128 --method pixeloe          # unquantised
./target/release/pixplusplus art.png -o out.png -s 128 --method pixeloe -k 16    # k-means + dither
#   --oe-thickness 3  --oe-mode contrast|k-centroid|lanczos|nearest|bilinear|...
#   --oe-sharpen unsharp|laplacian  --oe-dither ordered|error-diffusion|none  --oe-no-match
#   --oe-post-match   upstream's colour fix after -k (output can then exceed k colours)
```

Colour controls, for both methods (1 = unchanged):

```sh
#   --saturation 1.3       output chroma (default: PIA 1.1, the paper's boost; PixelOE 1.0)
#   --contrast 1.2         output lightness stretched around the image's mean lightness
#   --pre-saturation 1.5   the same, applied to the input before the palette is chosen,
#   --pre-contrast 1.2     so the palette itself picks more vivid or contrasty colours
```

Palette (`--palette`, K1, K5):

```sh
#   --palette native           the method's own palette (default): PIA's solver, PixelOE's k-means
#   --palette decimate-oklab   merge the closest colours, keeping the more common one, then map
#   --palette decimate-rgb     each pixel to its nearest colour with no dither; keeps small vivid
#                              colours that averaging would grey out. Oklab measures closeness as
#                              the eye sees it (cleaner, lowest error on the showcase images); RGB
#                              keeps the most vivid colours but can leave stray bright pixels.
#                              PixelOE: needs -k. PIA: about 3x slower.
```

`-k` is an upper limit. The output adjustments never add colours, though strong settings can
merge some (saturation and contrast 0 give a single grey). The one exception is PixelOE's
`--oe-post-match`, which brings back continuous tones and can exceed `-k`.

Notes:

- `--input-factor` (default 4) sets how big the working input is relative to the output.
  - PIA downscales the input to at most that many times the output; 0 uses the input as is.
  - PixelOE rounds the factor to a whole pixel size and resizes the input to exactly that many
    times the output. With 0, the input is used as is and the pixel size is the input's long
    side divided by `-s`, rounded.
- PIA never enlarges an input: if the output grid is larger than the image, the grid is scaled
  down to fit (aspect kept) with a warning.

`pixplusplus --help` lists every flag.

## Parity

PIA has a compat mode (`--pyxeled-compat`) that reproduces
[Pyxeled](https://github.com/garretthoffman/pyxeled); PixelOE is checked against upstream.
Both scripts use the Python venv in `bench/.venv`:

```sh
bench/.venv/bin/python bench/parity.py <source> <pyxeled_out> <pixplusplus_out>...
bench/.venv/bin/python bench/pixeloe_parity.py
```

Every divergence and the measured parity numbers are in [`docs/PORTING.md`](docs/PORTING.md).

## Development

```sh
cargo test
cargo clippy                  # must stay warning-free (also with --features gui)
cargo fmt --check
```

Developed with [DLAD](https://github.com/kritzerenkrieg/dlad): [`DECISIONS.md`](DECISIONS.md) is the
source of truth, then [`ARCHITECTURE.md`](ARCHITECTURE.md) and [`ROADMAP.md`](ROADMAP.md).
Agent entry point: [`AGENTS.md`](AGENTS.md).

## License

Pix++ is dual-licensed under MIT or Apache-2.0, at your option (see `Cargo.toml`).
The PixelOE port keeps upstream's Apache-2.0 licence: [`LICENSES/`](LICENSES/).
The sample image's source is credited in [`bench/images/ATTRIBUTION.md`](bench/images/ATTRIBUTION.md).
