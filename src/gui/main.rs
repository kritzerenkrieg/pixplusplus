//! `pixplusplus-gui`: desktop front end for Pix++ (J1–J6). egui/eframe on Linux and Windows
//! (J5). Jobs run on a worker thread with a spinner (J4); the output is the same as the
//! CLI's for the same settings (J2).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod profiles;
mod settings;

use eframe::egui;
use egui::{Color32, ComboBox, RichText, TextureHandle, TextureOptions};
use image::RgbImage;
use pixplusplus::pixeloe::color::Dither;
use pixplusplus::pixeloe::downscale::Interp;
use pixplusplus::pixeloe::outline::{Mapping, Normalize};
use pixplusplus::pixeloe::pipeline::{Mode, Quant, Sharpen};
use pixplusplus::F;
use settings::{Method, PaletteChoice, RunOutput, Settings};
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
    /// The settings that made it, to flag a result that no longer matches the panel.
    settings: Settings,
    colours: usize,
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

/// Profile state (L1).
struct Profiles {
    /// The profiles folder; `None` when the config folder is unknown.
    dir: Option<PathBuf>,
    names: Vec<String>,
    /// The name picked in the list.
    selected: Option<String>,
    /// The profile last loaded or saved, and its settings then (for the unsaved-changes mark).
    current: Option<(String, Settings)>,
    dialog: Option<Dialog>,
}

enum Dialog {
    /// `focus`: put the cursor in the name field on the frame the dialog opens, then leave
    /// focus to the user (a request every frame interrupts IME input).
    SaveAs {
        name: String,
        error: Option<String>,
        focus: bool,
    },
    ConfirmOverwrite(String),
    ConfirmDelete(String),
}

impl Profiles {
    fn new() -> Self {
        let dir = profiles::default_dir();
        let names = dir.as_deref().map(profiles::list).unwrap_or_default();
        Self {
            dir,
            names,
            selected: None,
            current: None,
            dialog: None,
        }
    }

    fn refresh(&mut self) {
        self.names = self.dir.as_deref().map(profiles::list).unwrap_or_default();
        if let Some(s) = &self.selected {
            if !self.names.contains(s) {
                self.selected = None;
            }
        }
    }
}

struct App {
    settings: Settings,
    profiles: Profiles,
    source: Option<Source>,
    result: Option<Result_>,
    view: View,
    status: Status,
    job: Option<Receiver<Result<RunOutput, String>>>,
    /// The settings of the job in flight.
    job_settings: Option<Settings>,
    /// A PNG export in flight (upscaling and encoding can take seconds at large scales).
    export: Option<Receiver<Result<PathBuf, String>>>,
    save_scale: u32,
}

impl Default for App {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            profiles: Profiles::new(),
            source: None,
            result: None,
            view: View::Source,
            status: Status::Idle,
            job: None,
            job_settings: None,
            export: None,
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
        self.job_settings = Some(self.settings.clone());
        self.status = Status::Idle;
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.export {
            use std::sync::mpsc::TryRecvError;
            match rx.try_recv() {
                Ok(r) => {
                    self.export = None;
                    self.status = match r {
                        Ok(path) => Status::Info(format!("Saved {}", path.display())),
                        Err(e) => Status::Error(e),
                    };
                }
                // The worker died before reporting (a panic): unlock the window.
                Err(TryRecvError::Disconnected) => {
                    self.export = None;
                    self.status = Status::Error("Saving failed unexpectedly.".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let Some(rx) = &self.job else { return };
        let Ok(r) = rx.try_recv() else { return };
        self.job = None;
        let settings = self.job_settings.take().unwrap_or_default();
        match r {
            Ok(out) => {
                self.status = match out.warning {
                    Some(w) => Status::Warning(format!("{}  ·  {w}", out.summary)),
                    None => Status::Info(out.summary),
                };
                let colours = out
                    .image
                    .pixels()
                    .map(|p| p.0)
                    .collect::<std::collections::HashSet<_>>()
                    .len();
                self.result = Some(Result_ {
                    image: out.image,
                    texture: None,
                    settings,
                    colours,
                });
                self.view = View::Result;
            }
            Err(e) => self.status = Status::Error(e),
        }
    }

    fn save_dialog(&mut self, ctx: &egui::Context) {
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
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
        {
            path.set_extension("png");
            // The dialog confirmed a different name; don't overwrite a file nobody confirmed.
            if path.exists() {
                self.status = Status::Error(format!(
                    "{} already exists. Pick it in the save dialog to overwrite it.",
                    path.display()
                ));
                return;
            }
        }
        // Upscale and encode on a worker thread so the window keeps repainting.
        let (img, scale, ctx) = (res.image.clone(), self.save_scale, ctx.clone());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let img = if scale > 1 {
                pixplusplus::job::upscale(&img, scale)
            } else {
                img
            };
            let r = img
                .save(&path)
                .map(|()| path.clone())
                .map_err(|e| format!("Could not save {}: {e}", path.display()));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.export = Some(rx);
    }

    /// Writes the current settings as profile `name` (overwrite already confirmed).
    fn save_profile(&mut self, name: &str) {
        let Some(dir) = self.profiles.dir.clone() else {
            self.status = Status::Error("No config folder found for profiles.".into());
            return;
        };
        match profiles::save(&dir, name, &self.settings) {
            Ok(()) => {
                let name = profiles::check_name(name).unwrap_or(name).to_owned();
                self.profiles.refresh();
                self.profiles.selected = Some(name.clone());
                self.profiles.current = Some((name.clone(), self.settings.clone()));
                self.status = Status::Info(format!("Saved profile \"{name}\""));
            }
            Err(e) => self.status = Status::Error(format!("Could not save the profile: {e}")),
        }
    }

    fn load_profile(&mut self, name: &str) {
        let Some(dir) = self.profiles.dir.clone() else {
            return;
        };
        match profiles::load(&dir, name) {
            Ok(s) => {
                self.settings = s.clone();
                self.profiles.current = Some((name.to_owned(), s));
                self.status = Status::Info(format!("Loaded profile \"{name}\""));
            }
            Err(e) => self.status = Status::Error(format!("Could not load the profile: {e}")),
        }
    }

    fn delete_profile(&mut self, name: &str) {
        let Some(dir) = self.profiles.dir.clone() else {
            return;
        };
        match profiles::delete(&dir, name) {
            Ok(()) => {
                if self
                    .profiles
                    .current
                    .as_ref()
                    .is_some_and(|(n, _)| n == name)
                {
                    self.profiles.current = None;
                }
                self.profiles.refresh();
                self.status = Status::Info(format!("Deleted profile \"{name}\""));
            }
            Err(e) => self.status = Status::Error(format!("Could not delete the profile: {e}")),
        }
    }

    /// The Profile section: pick, load, save, save as, delete (L1).
    fn profile_ui(&mut self, ui: &mut egui::Ui) {
        let Some(dir) = self.profiles.dir.clone() else {
            ui.label(RichText::new("No config folder found; profiles are off.").weak());
            return;
        };
        let current = match &self.profiles.current {
            Some((n, saved)) if *saved == self.settings => n.clone(),
            Some((n, _)) => format!("{n} *"),
            None => "Unsaved settings".into(),
        };
        ui.label(RichText::new(current).weak())
            .on_hover_text("The profile in use. * = changed since it was loaded or saved.");

        let shown = self
            .profiles
            .selected
            .clone()
            .unwrap_or_else(|| "Choose a profile…".into());
        ui.horizontal(|ui| {
            ComboBox::from_id_salt("profiles")
                .width(ui.available_width() - 60.0)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    if self.profiles.names.is_empty() {
                        ui.label(RichText::new("No saved profiles yet").weak());
                    }
                    for n in &self.profiles.names {
                        ui.selectable_value(&mut self.profiles.selected, Some(n.clone()), n);
                    }
                })
                .response
                .on_hover_text(format!("Profiles live in {}", dir.display()));
            let picked = self.profiles.selected.clone();
            if ui
                .add_enabled(picked.is_some(), egui::Button::new("Load"))
                .on_hover_text("Replace the current settings with this profile.")
                .clicked()
            {
                if let Some(n) = picked {
                    self.load_profile(&n);
                }
            }
        });
        ui.horizontal(|ui| {
            let target = self.profiles.current.as_ref().map(|(n, _)| n.clone());
            let save = ui.button("Save").on_hover_text(match &target {
                Some(n) => format!("Overwrite \"{n}\" with the current settings."),
                None => "Save the current settings as a new profile.".into(),
            });
            if save.clicked() {
                match target {
                    Some(n) => self.save_profile(&n),
                    None => {
                        self.profiles.dialog = Some(Dialog::SaveAs {
                            name: String::new(),
                            error: None,
                            focus: true,
                        })
                    }
                }
            }
            if ui
                .button("Save as…")
                .on_hover_text("Save the current settings under a new name.")
                .clicked()
            {
                let name = self
                    .profiles
                    .current
                    .as_ref()
                    .map(|(n, _)| n.clone())
                    .unwrap_or_default();
                self.profiles.dialog = Some(Dialog::SaveAs {
                    name,
                    error: None,
                    focus: true,
                });
            }
            let picked = self.profiles.selected.clone();
            if ui
                .add_enabled(picked.is_some(), egui::Button::new("Delete"))
                .on_hover_text("Delete the profile picked in the list.")
                .clicked()
            {
                if let Some(n) = picked {
                    self.profiles.dialog = Some(Dialog::ConfirmDelete(n));
                }
            }
        });
    }

    /// Save-as and confirmation dialogs (L1).
    fn profile_dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.profiles.dialog.take() else {
            return;
        };
        let Some(dir) = self.profiles.dir.clone() else {
            return;
        };
        let window = |title: &str| {
            egui::Window::new(title.to_owned())
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        };
        let mut next = None;
        match dialog {
            Dialog::SaveAs {
                mut name,
                mut error,
                focus,
            } => {
                window("Save profile as").show(ctx, |ui| {
                    ui.label("Profile name:");
                    let edit = ui.text_edit_singleline(&mut name);
                    if focus {
                        edit.request_focus();
                    }
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if let Some(e) = &error {
                        ui.colored_label(Color32::from_rgb(220, 80, 80), e);
                    }
                    let (mut ok, mut cancel) = (enter, false);
                    ui.horizontal(|ui| {
                        ok |= ui.button("Save").clicked();
                        cancel = ui.button("Cancel").clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Escape));
                    });
                    if cancel {
                        return;
                    }
                    if ok {
                        match profiles::check_name(&name) {
                            Err(e) => error = Some(e),
                            Ok(n) if profiles::exists(&dir, n) => {
                                next = Some(Dialog::ConfirmOverwrite(n.to_owned()));
                                return;
                            }
                            Ok(n) => {
                                let n = n.to_owned();
                                self.save_profile(&n);
                                return;
                            }
                        }
                    }
                    next = Some(Dialog::SaveAs {
                        name: name.clone(),
                        error: error.clone(),
                        focus: false,
                    });
                });
            }
            Dialog::ConfirmOverwrite(n) => {
                window("Replace profile?").show(ctx, |ui| {
                    ui.label(format!(
                        "A profile named \"{n}\" already exists. Replace it?"
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Replace").clicked() {
                            self.save_profile(&n);
                        } else if ui.button("Cancel").clicked() {
                        } else {
                            next = Some(Dialog::ConfirmOverwrite(n.clone()));
                        }
                    });
                });
            }
            Dialog::ConfirmDelete(n) => {
                window("Delete profile?").show(ctx, |ui| {
                    ui.label(format!("Delete the profile \"{n}\"? This can't be undone."));
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            self.delete_profile(&n);
                        } else if ui.button("Cancel").clicked() {
                        } else {
                            next = Some(Dialog::ConfirmDelete(n.clone()));
                        }
                    });
                });
            }
        }
        if self.profiles.dialog.is_none() {
            self.profiles.dialog = next;
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        section(ui, "Method", true, |ui| {
            for m in Method::ALL {
                ui.radio_value(&mut s.method, m, RichText::new(m.label()).strong())
                    .on_hover_text(format!("--method {}", m.short().to_lowercase()));
                ui.indent(m.short(), |ui| {
                    ui.label(RichText::new(m.blurb()).weak());
                });
            }
        });

        section(ui, "Output", true, |ui| {
            egui::Grid::new("output")
                .num_columns(2)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Size")
                        .on_hover_text("-s: longest side of the result, in pixels.");
                    ui.add(
                        egui::DragValue::new(&mut s.size)
                            .range(4..=1024)
                            .suffix(" px"),
                    );
                    ui.end_row();

                    if s.method == Method::Pixeloe {
                        ui.checkbox(&mut s.oe_quantise, "Colours").on_hover_text(
                            "-k: limit the palette. Off: keep every colour PixelOE produces.",
                        );
                        ui.add_enabled(
                            s.oe_quantise,
                            egui::DragValue::new(&mut s.colors).range(2..=256),
                        )
                        .on_disabled_hover_text("Tick Colours to limit the palette.");
                    } else {
                        ui.label("Colours")
                            .on_hover_text("-k: at most this many colours.");
                        ui.add(egui::DragValue::new(&mut s.colors).range(2..=256));
                    }
                    ui.end_row();

                    let limited = s.method != Method::Pixeloe || s.oe_quantise;
                    ui.label("Palette").on_hover_text(
                        "--palette. Native: the method's own palette. Decimate: merge the \
                         closest colours, keeping the more common one, with no dither; keeps \
                         small vivid colours. Oklab: closeness as the eye sees it (cleaner). \
                         RGB: most vivid, can leave stray bright pixels.",
                    );
                    ui.add_enabled_ui(limited, |ui| {
                        combo(
                            ui,
                            "palette",
                            &mut s.palette,
                            &[
                                (PaletteChoice::Native, "Native"),
                                (PaletteChoice::DecimateOklab, "Decimate (Oklab)"),
                                (PaletteChoice::DecimateRgb, "Decimate (RGB)"),
                            ],
                        );
                    })
                    .response
                    .on_disabled_hover_text("Limit the colours first.");
                    ui.end_row();
                });
            if s.method == Method::Pia {
                ui.add_space(2.0);
                ui.checkbox(&mut s.importance, "Importance map")
                    .on_hover_text(
                        "--importance auto: finds the eye-catching parts of the image \
                         (spectral-residual saliency) and gives them more of the palette. \
                         Untick for plain PIA.",
                    );
            }
        });

        section(ui, "Colour", true, |ui| colour_ui(ui, s));

        if s.method == Method::Pixeloe {
            section(ui, "PixelOE options", false, |ui| pixeloe_ui(ui, s));
        }
    }
}

/// A titled, collapsible block of the settings panel; remembers whether it is open.
fn section(ui: &mut egui::Ui, title: &str, open: bool, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).strong())
        .default_open(open)
        .show(ui, |ui| {
            add(ui);
            ui.add_space(4.0);
        });
}

/// Saturation and contrast before and after the palette (K4).
fn colour_ui(ui: &mut egui::Ui, s: &mut Settings) {
    let saturation = if s.method == Method::Pixeloe {
        &mut s.oe_saturation
    } else {
        &mut s.pia_saturation
    };
    fn slider(v: &mut F) -> egui::Slider<'_> {
        egui::Slider::new(v, 0.0..=3.0)
            .step_by(0.05)
            .fixed_decimals(2)
    }
    ui.label(RichText::new("Result").weak());
    egui::Grid::new("colour_post")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Saturation").on_hover_text(
                "--saturation: colourfulness of the result. 1 = unchanged, 0 = grey. \
                 PIA's default 1.1 is the paper's palette boost.",
            );
            ui.add(slider(saturation));
            ui.end_row();
            ui.label("Contrast").on_hover_text(
                "--contrast: lights lighter and darks darker around the image's average. \
                 Never adds colours.",
            );
            ui.add(slider(&mut s.contrast));
            ui.end_row();
        });
    ui.add_space(4.0);
    ui.label(RichText::new("Before the palette is chosen").weak())
        .on_hover_text(
            "Changes which colours the palette picks: raise saturation to keep more vivid \
             colours, contrast for more light/dark separation.",
        );
    egui::Grid::new("colour_pre")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Saturation")
                .on_hover_text("--pre-saturation, applied to the input.");
            ui.add(slider(&mut s.pre.saturation));
            ui.end_row();
            ui.label("Contrast")
                .on_hover_text("--pre-contrast, applied to the input.");
            ui.add(slider(&mut s.pre.contrast));
            ui.end_row();
        });
    ui.add_space(2.0);
    let d = Settings::default();
    let changed = (s.pia_saturation, s.oe_saturation, s.contrast, s.pre)
        != (d.pia_saturation, d.oe_saturation, d.contrast, d.pre);
    if ui
        .add_enabled(changed, egui::Button::new("Reset colour").small())
        .on_hover_text("Back to the defaults.")
        .clicked()
    {
        (s.pia_saturation, s.oe_saturation) = (d.pia_saturation, d.oe_saturation);
        (s.contrast, s.pre) = (d.contrast, d.pre);
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
            ui.add(egui::DragValue::new(&mut o.thickness).range(0..=6));
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
    let max = ctx.input(|i| i.max_texture_side) as u32;
    let (w, h) = img.dimensions();
    let shrunk;
    let img = if w > max || h > max {
        let s = max as f64 / w.max(h) as f64;
        let (tw, th) = (
            ((w as f64 * s) as u32).clamp(1, max),
            ((h as f64 * s) as u32).clamp(1, max),
        );
        shrunk = image::imageops::resize(img, tw, th, image::imageops::FilterType::Triangle);
        &shrunk
    } else {
        img
    };
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

impl App {
    /// Keyboard shortcuts: Ctrl+O open, Ctrl+Enter run, Ctrl+S save.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        if self.job.is_some() || self.export.is_some() || self.profiles.dialog.is_some() {
            return;
        }
        let pressed = |k: Key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, k)))
        };
        if pressed(Key::O) {
            self.open_dialog();
        }
        if pressed(Key::Enter) && self.source.is_some() {
            self.start(ctx);
        }
        if pressed(Key::S) && self.result.is_some() {
            self.save_dialog(ctx);
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, running: bool) {
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!running, |ui| {
                if ui
                    .button("📂 Open…")
                    .on_hover_text("Open an image (Ctrl+O). You can also drop one on the window.")
                    .clicked()
                {
                    self.open_dialog();
                }
                let has_result = self.result.is_some();
                if ui
                    .add_enabled(has_result, egui::Button::new("💾 Save PNG…"))
                    .on_hover_text("Save the result (Ctrl+S).")
                    .on_disabled_hover_text("Run first, then save the result.")
                    .clicked()
                {
                    self.save_dialog(&ui.ctx().clone());
                }
                ui.add_enabled(
                    has_result,
                    egui::DragValue::new(&mut self.save_scale)
                        .range(1..=32)
                        .prefix("at ×"),
                )
                .on_hover_text("Scale the saved PNG up by whole pixels, so it stays sharp.");
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(self.result.is_some(), |ui| {
                    ui.selectable_value(&mut self.view, View::Result, "Result")
                        .on_hover_text("Hold Space to peek at the source.");
                });
                ui.add_enabled_ui(self.source.is_some(), |ui| {
                    ui.selectable_value(&mut self.view, View::Source, "Source");
                });
                ui.label(RichText::new("View").weak());
            });
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui, running: bool) {
        ui.horizontal(|ui| {
            if running {
                ui.spinner();
                ui.label(if self.export.is_some() {
                    "Saving…"
                } else {
                    "Working…"
                });
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let info = match self.view {
                    View::Result => self.result.as_ref().map(|r| {
                        format!(
                            "{}×{} · {} colours",
                            r.image.width(),
                            r.image.height(),
                            r.colours
                        )
                    }),
                    View::Source => self
                        .source
                        .as_ref()
                        .map(|s| format!("{}×{} source", s.image.width(), s.image.height())),
                };
                if let Some(t) = info {
                    ui.label(RichText::new(t).weak());
                }
            });
        });
    }

    /// The settings column: profile and settings scroll; Run stays pinned at the bottom.
    fn side_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, running: bool) {
        let modal = self.profiles.dialog.is_some();
        egui::Panel::bottom("run")
            .show_separator_line(true)
            .show(ui, |ui| {
                ui.add_space(6.0);
                let can_run = self.source.is_some() && !running && !modal;
                let label = match (running, self.export.is_some()) {
                    (_, true) => "Saving…",
                    (true, false) => "Working…",
                    _ => "▶  Run",
                };
                let run = ui
                    .add_enabled(
                        can_run,
                        egui::Button::new(RichText::new(label).strong().size(16.0))
                            .min_size(egui::vec2(ui.available_width(), 34.0)),
                    )
                    .on_hover_text("Make the pixel art (Ctrl+Enter).")
                    .on_disabled_hover_text("Open an image first.");
                if run.clicked() {
                    self.start(ctx);
                }
                ui.add_space(4.0);
            });
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_enabled_ui(!running && !modal, |ui| {
                section(ui, "Profile", true, |ui| self.profile_ui(ui));
                self.settings_ui(ui);
            });
        });
    }

    fn image_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let peek = self.view == View::Result
            && !ctx.egui_wants_keyboard_input()
            && ctx.input(|i| i.key_down(egui::Key::Space));
        let view = if peek { View::Source } else { self.view };
        let stale = self
            .result
            .as_ref()
            .is_some_and(|r| r.settings != self.settings);
        if view == View::Result && stale && self.job.is_none() && self.export.is_none() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Settings changed since this result.")
                        .color(Color32::from_rgb(0xd8, 0xa0, 0x20)),
                );
                if ui.small_button("Run again").clicked() {
                    self.start(ctx);
                }
                ui.label(RichText::new("Ctrl+Enter").weak().small());
            });
        }
        match view {
            View::Result => {
                if let Some(r) = &mut self.result {
                    let tex = r.texture.get_or_insert_with(|| {
                        to_texture(ctx, "result", &r.image, TextureOptions::NEAREST)
                    });
                    show_image(ui, tex, true);
                }
            }
            View::Source => match &mut self.source {
                Some(s) => {
                    let tex = s.texture.get_or_insert_with(|| {
                        to_texture(ctx, "source", &s.image, TextureOptions::LINEAR)
                    });
                    show_image(ui, tex, false);
                }
                None => {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            RichText::new(
                                "Drop an image here, or click Open… (Ctrl+O)\n\n\
                                 Then pick a method on the left and press Run.",
                            )
                            .size(17.0)
                            .weak(),
                        );
                    });
                }
            },
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        self.shortcuts(&ctx);
        // Settings and file actions wait for both the job and an export in flight.
        let running = self.job.is_some() || self.export.is_some();

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
            ui.add_space(2.0);
            self.toolbar(ui, running);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui, running));
        egui::Panel::left("settings")
            .resizable(true)
            .default_size(300.0)
            .size_range(260.0..=440.0)
            .show(ui, |ui| self.side_panel(ui, &ctx, running));
        self.profile_dialogs(&ctx);
        egui::CentralPanel::default().show(ui, |ui| self.image_view(ui, &ctx));
    }
}
