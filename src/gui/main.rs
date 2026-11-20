//! `pixplusplus-gui`: desktop front end for Pix++ (J1–J6). egui/eframe on Linux, Windows and
//! macOS (J5). Jobs run on a worker thread with a spinner (J4); the output is the same as the
//! CLI's for the same settings (J2).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod settings;

use eframe::egui;
use egui::{Color32, ComboBox, RichText, TextureHandle, TextureOptions};
use image::RgbImage;
use pixplusplus::pixeloe::color::Dither;
use pixplusplus::pixeloe::downscale::Interp;
use pixplusplus::pixeloe::outline::{Mapping, Normalize};
use pixplusplus::pixeloe::pipeline::{Mode, Quant, Sharpen};
use settings::{ImportanceSource, Method, RunOutput, Settings};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

const IMAGE_EXTS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Pix++")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([640.0, 420.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let start = std::env::args_os().nth(1).map(PathBuf::from);
    eframe::run_native(
        "Pix++",
        options,
        Box::new(move |_cc| {
            let mut app = App::default();
            if let Some(p) = start {
                app.open(&p);
            }
            Ok(Box::new(app))
        }),
    )
}

struct Source {
    path: PathBuf,
    image: Arc<RgbImage>,
    texture: Option<TextureHandle>,
}

struct Result_ {
    image: RgbImage,
    texture: Option<TextureHandle>,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum View {
    Source,
    Result,
}

#[derive(Default)]
enum Status {
    #[default]
    Idle,
    Info(String),
    Warning(String),
    Error(String),
}

struct App {
    settings: Settings,
    source: Option<Source>,
    result: Option<Result_>,
    view: View,
    status: Status,
    job: Option<Receiver<Result<RunOutput, String>>>,
    save_scale: u32,
}

impl Default for App {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            source: None,
            result: None,
            view: View::Source,
            status: Status::Idle,
            job: None,
            save_scale: 1,
        }
    }
}

impl App {
    fn open(&mut self, path: &Path) {
        match image::open(path) {
            Ok(img) => {
                self.source = Some(Source {
                    path: path.to_path_buf(),
                    image: Arc::new(img.to_rgb8()),
                    texture: None,
                });
                self.result = None;
                self.view = View::Source;
                self.status = Status::Info(format!("Opened {}", path.display()));
            }
            Err(e) => {
                self.status = Status::Error(format!("Could not open {}: {e}", path.display()))
            }
        }
    }

    fn open_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("Images", &IMAGE_EXTS)
            .pick_file()
        {
            self.open(&p);
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let Some(src) = &self.source else { return };
        let (tx, rx) = channel();
        let img = (*src.image).clone();
        let settings = self.settings.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| settings.run(img)))
                .unwrap_or_else(|e| {
                    let msg = e
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "unknown error".into());
                    Err(format!("The run failed: {msg}"))
                });
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.job = Some(rx);
        self.status = Status::Idle;
    }

    fn poll(&mut self) {
        let Some(rx) = &self.job else { return };
        let Ok(r) = rx.try_recv() else { return };
        self.job = None;
        match r {
            Ok(out) => {
                self.status = match out.warning {
                    Some(w) => Status::Warning(format!("{}  ·  {w}", out.summary)),
                    None => Status::Info(out.summary),
                };
                self.result = Some(Result_ {
                    image: out.image,
                    texture: None,
                });
                self.view = View::Result;
            }
            Err(e) => self.status = Status::Error(e),
        }
    }

    fn save_dialog(&mut self) {
        let (Some(res), Some(src)) = (&self.result, &self.source) else {
            return;
        };
        let stem = src
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".into());
        let Some(mut path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .set_file_name(format!("{stem}_pix.png"))
            .save_file()
        else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("png");
        }
        let img = if self.save_scale > 1 {
            pixplusplus::job::upscale(&res.image, self.save_scale)
        } else {
            res.image.clone()
        };
        self.status = match img.save(&path) {
            Ok(()) => Status::Info(format!("Saved {}", path.display())),
            Err(e) => Status::Error(format!("Could not save {}: {e}", path.display())),
        };
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.heading("Method");
        for m in Method::ALL {
            ui.radio_value(&mut s.method, m, m.label());
        }
        ui.separator();

        egui::Grid::new("common")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Size (long side)")
                    .on_hover_text("-s: output long side in pixels, aspect kept.");
                ui.add(
                    egui::DragValue::new(&mut s.size)
                        .range(4..=1024)
                        .suffix(" px"),
                );
                ui.end_row();
                if s.method == Method::Pixeloe {
                    ui.checkbox(&mut s.oe_quantise, "Limit colours")
                        .on_hover_text("-k: quantise with k-means (G8). Off: unquantised.");
                    ui.add_enabled(
                        s.oe_quantise,
                        egui::DragValue::new(&mut s.colors).range(2..=256),
                    );
                } else {
                    ui.label("Colours")
                        .on_hover_text("-k: palette size, a hard limit (B4).");
                    ui.add(egui::DragValue::new(&mut s.colors).range(2..=256));
                }
                ui.end_row();
            });

        if s.method == Method::PiaImportance {
            ui.separator();
            ui.heading("Importance map");
            let auto = s.importance == ImportanceSource::Auto;
            if ui.radio(auto, "Automatic (saliency)").clicked() {
                s.importance = ImportanceSource::Auto;
            }
            ui.horizontal(|ui| {
                let file = match &s.importance {
                    ImportanceSource::File(p) => p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    ImportanceSource::Auto => "none".into(),
                };
                let _ = ui.radio(!auto, "File:");
                if ui
                    .button(file)
                    .on_hover_text("Grayscale image, white = more palette mass.")
                    .clicked()
                {
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter("Images", &IMAGE_EXTS)
                        .pick_file()
                    {
                        s.importance = ImportanceSource::File(p);
                    }
                }
            });
        }

        if s.method == Method::Pixeloe {
            ui.separator();
            ui.heading("PixelOE");
            pixeloe_ui(ui, s);
        }
    }
}

fn combo<T: PartialEq + Copy>(ui: &mut egui::Ui, id: &str, value: &mut T, items: &[(T, &str)]) {
    let current = items
        .iter()
        .find(|(v, _)| v == value)
        .map(|(_, l)| *l)
        .unwrap_or("?");
    ComboBox::from_id_salt(id)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (v, l) in items {
                ui.selectable_value(value, *v, *l);
            }
        });
}

/// The `--oe-*` options (G9).
fn pixeloe_ui(ui: &mut egui::Ui, s: &mut Settings) {
    let o = &mut s.oe;
    egui::Grid::new("pixeloe")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Outline thickness")
                .on_hover_text("--oe-thickness (0 = no outline expansion)");
            ui.add(egui::DragValue::new(&mut o.thickness).range(0..=10));
            ui.end_row();

            ui.label("Downscale mode").on_hover_text("--oe-mode");
            combo(
                ui,
                "mode",
                &mut o.mode,
                &[
                    (Mode::Contrast, "contrast"),
                    (Mode::KCentroid, "k-centroid"),
                    (Mode::Lanczos, "lanczos"),
                    (Mode::Interp(Interp::Nearest), "nearest"),
                    (Mode::Interp(Interp::NearestExact), "nearest-exact"),
                    (Mode::Interp(Interp::Bilinear), "bilinear"),
                    (Mode::Interp(Interp::Bicubic), "bicubic"),
                    (Mode::Interp(Interp::Area), "area"),
                ],
            );
            ui.end_row();

            ui.label("Sharpen").on_hover_text("--oe-sharpen");
            combo(
                ui,
                "sharpen",
                &mut o.sharpen,
                &[
                    (None, "none"),
                    (Some(Sharpen::Unsharp), "unsharp"),
                    (Some(Sharpen::Laplacian), "laplacian"),
                ],
            );
            ui.end_row();

            ui.label("Sharpen amount")
                .on_hover_text("--oe-sharpen-factor");
            ui.add_enabled(
                o.sharpen.is_some(),
                egui::DragValue::new(&mut o.sharpen_factor)
                    .range(0.0..=4.0)
                    .speed(0.01),
            );
            ui.end_row();

            ui.label("Match colours")
                .on_hover_text("Off = --oe-no-match");
            ui.checkbox(&mut o.color_match, "");
            ui.end_row();

            ui.label("Outline weights").on_hover_text("--oe-mapping");
            combo(
                ui,
                "mapping",
                &mut o.mapping,
                &[
                    (Mapping::Current, "current"),
                    (Mapping::ContrastRatio, "contrast-ratio"),
                    (Mapping::ContrastGated, "contrast-gated"),
                ],
            );
            ui.end_row();

            ui.label("Weight normalise").on_hover_text("--oe-normalize");
            combo(
                ui,
                "normalize",
                &mut o.normalize,
                &[(Normalize::Global, "global"), (Normalize::None, "none")],
            );
            ui.end_row();
        });

    ui.add_enabled_ui(s.oe_quantise, |ui| {
        ui.label(RichText::new("When limiting colours").weak());
        egui::Grid::new("pixeloe_quant")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Quantiser").on_hover_text("--oe-quant");
                combo(
                    ui,
                    "quant",
                    &mut o.quant_mode,
                    &[
                        (Quant::Kmeans, "k-means"),
                        (Quant::WeightedKmeans, "weighted k-means"),
                        (Quant::RepeatKmeans, "repeat k-means"),
                    ],
                );
                ui.end_row();

                ui.label("Dither").on_hover_text("--oe-dither");
                combo(
                    ui,
                    "dither",
                    &mut o.dither,
                    &[
                        (Dither::Ordered, "ordered"),
                        (Dither::ErrorDiffusion, "error diffusion"),
                        (Dither::None, "none"),
                    ],
                );
                ui.end_row();

                ui.label("Post match").on_hover_text(
                    "--oe-post-match: upstream's colour match after quantising; \
                     the output then has more colours than the limit (G10).",
                );
                ui.checkbox(&mut o.post_match, "");
                ui.end_row();
            });
    });
}

fn to_texture(
    ctx: &egui::Context,
    name: &str,
    img: &RgbImage,
    opt: TextureOptions,
) -> TextureHandle {
    let ci =
        egui::ColorImage::from_rgb([img.width() as usize, img.height() as usize], img.as_raw());
    ctx.load_texture(name, ci, opt)
}

/// Draws `tex` centred, as large as fits. `pixel_art` snaps to whole-number zoom.
fn show_image(ui: &mut egui::Ui, tex: &TextureHandle, pixel_art: bool) {
    let avail = ui.available_size();
    let size = tex.size_vec2();
    let mut z = (avail.x / size.x).min(avail.y / size.y);
    if pixel_art && z >= 1.0 {
        z = z.floor();
    }
    let shown = size * z.max(0.01);
    ui.centered_and_justified(|ui| {
        ui.add(egui::Image::from_texture((tex.id(), shown)).fit_to_exact_size(shown));
    });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        let running = self.job.is_some();

        if let Some(p) = ctx.input(|i| {
            i.raw.dropped_files.iter().find_map(|f| {
                let p = f.path();
                (!p.as_os_str().is_empty()).then(|| p.to_path_buf())
            })
        }) {
            if !running {
                self.open(&p);
            }
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!running, |ui| {
                    if ui.button("Open…").clicked() {
                        self.open_dialog();
                    }
                    let can_run = self.source.is_some();
                    if ui
                        .add_enabled(can_run, egui::Button::new(RichText::new("▶ Run").strong()))
                        .clicked()
                    {
                        self.start(&ctx);
                    }
                    ui.separator();
                    let has_result = self.result.is_some();
                    if ui
                        .add_enabled(has_result, egui::Button::new("Save PNG…"))
                        .clicked()
                    {
                        self.save_dialog();
                    }
                    ui.label("scale");
                    ui.add(
                        egui::DragValue::new(&mut self.save_scale)
                            .range(1..=32)
                            .prefix("×"),
                    )
                    .on_hover_text("Nearest-neighbour upscale when saving (like --preview).");
                });
                ui.separator();
                ui.selectable_value(&mut self.view, View::Source, "Source");
                ui.add_enabled_ui(self.result.is_some(), |ui| {
                    ui.selectable_value(&mut self.view, View::Result, "Result");
                });
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if running {
                    ui.spinner();
                    ui.label("Running…");
                } else {
                    match &self.status {
                        Status::Idle => {
                            ui.label(RichText::new("Open or drop an image to start.").weak());
                        }
                        Status::Info(s) => {
                            ui.label(s);
                        }
                        Status::Warning(s) => {
                            ui.colored_label(Color32::from_rgb(0xd8, 0xa0, 0x20), s);
                        }
                        Status::Error(s) => {
                            ui.colored_label(Color32::from_rgb(0xe0, 0x50, 0x50), s);
                        }
                    }
                }
            });
        });

        egui::Panel::left("settings")
            .resizable(false)
            .exact_size(250.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_enabled_ui(!running, |ui| self.settings_ui(ui));
                });
            });

        egui::CentralPanel::default().show(ui, |ui| match self.view {
            View::Result => {
                if let Some(r) = &mut self.result {
                    let tex = r.texture.get_or_insert_with(|| {
                        to_texture(&ctx, "result", &r.image, TextureOptions::NEAREST)
                    });
                    show_image(ui, tex, true);
                }
            }
            View::Source => match &mut self.source {
                Some(s) => {
                    let tex = s.texture.get_or_insert_with(|| {
                        to_texture(&ctx, "source", &s.image, TextureOptions::LINEAR)
                    });
                    show_image(ui, tex, false);
                }
                None => {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            RichText::new("Drop an image here, or click Open…")
                                .size(18.0)
                                .weak(),
                        );
                    });
                }
            },
        });
    }
}
