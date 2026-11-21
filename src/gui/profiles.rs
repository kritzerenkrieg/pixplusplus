//! Named settings profiles (L1): one JSON file per profile in the platform config folder.
//!
//! File shape: `{"version": 3, "settings": {...}}`. Missing settings take the defaults
//! (`Settings` is `#[serde(default)]`), so profiles written by older builds keep loading.
//! Version 1 had three methods and an importance source; [`migrate_v1`] maps it (J7).
//! Before version 3 a missing palette meant native; [`migrate_v2`] keeps that (K6).

use crate::settings::Settings;
use std::path::{Path, PathBuf};

/// Profile file format version. 2: two methods, importance on/off (J7). 3: the default
/// palette is balanced (K6).
const VERSION: u32 = 3;
/// Longest profile name, in characters.
const MAX_NAME: usize = 64;

#[derive(serde::Serialize)]
struct File<'a> {
    version: u32,
    settings: &'a Settings,
}

/// Version 1 → 2, keeping what the profile produced. v1 methods: `pia` (no importance map),
/// `pia-importance` (map from `importance`: `"auto"` or `{"file": path}`), `pixeloe`. v2 has
/// `pia` with `importance: bool` (automatic only; a v1 file map becomes automatic).
fn migrate_v1(settings: &mut serde_json::Value) {
    let Some(s) = settings.as_object_mut() else {
        return;
    };
    let method = s
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("pia")
        .to_owned();
    let importance = match method.as_str() {
        "pia-importance" => {
            s.insert("method".into(), "pia".into());
            true
        }
        "pia" => false,
        _ => true,
    };
    s.insert("importance".into(), importance.into());
}

/// Version 2 → 3: the default palette became balanced (K6), so a profile that left the palette
/// out, meaning native when it was saved, says native.
fn migrate_v2(settings: &mut serde_json::Value) {
    if let Some(s) = settings.as_object_mut() {
        s.entry("palette").or_insert_with(|| "native".into());
    }
}

/// The profiles folder: `pixplusplus/profiles` in the platform config folder. `None` when the
/// home or config folder is unknown.
pub fn default_dir() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        env("APPDATA")?
    } else if cfg!(target_os = "macos") {
        env("HOME")?.join("Library").join("Application Support")
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))?
    };
    Some(base.join("pixplusplus").join("profiles"))
}

/// Checks a profile name: it becomes a file name, so no path parts or reserved characters.
pub fn check_name(name: &str) -> Result<&str, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("Enter a profile name.".into());
    }
    if n.chars().count() > MAX_NAME {
        return Err(format!("Use at most {MAX_NAME} characters."));
    }
    if n.starts_with('.')
        || n.ends_with('.')
        || n.chars()
            .any(|c| c.is_control() || "/\\<>:\"|?*".contains(c))
    {
        return Err("Avoid / \\ < > : \" | ? * and a leading or trailing dot.".into());
    }
    Ok(n)
}

fn path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    Ok(dir.join(format!("{}.json", check_name(name)?)))
}

/// Profile names in `dir`, sorted case-insensitively. A missing folder means no profiles.
pub fn list(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension()? == "json" && p.is_file())
                .then(|| p.file_stem()?.to_str().map(str::to_owned))
                .flatten()
        })
        .filter(|n| check_name(n).is_ok())
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

pub fn exists(dir: &Path, name: &str) -> bool {
    path(dir, name).is_ok_and(|p| p.is_file())
}

/// Writes the profile, replacing any with the same name (via a temporary file, so a failed
/// write leaves the old one intact).
pub fn save(dir: &Path, name: &str, settings: &Settings) -> Result<(), String> {
    let p = path(dir, name)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let json = serde_json::to_string_pretty(&File {
        version: VERSION,
        settings,
    })
    .map_err(|e| e.to_string())?;
    let tmp = staging_file(dir, "profile")?;
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).open(&tmp)?;
        f.write_all((json + "\n").as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &p)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", p.display())
    })
}

/// A new, empty staging file in `dir`, created exclusively, so two saves (say from two open
/// windows) never share one. The leading dot and `.tmp` keep it out of [`list`]. `tag` names
/// what is being written (`profile`, `png`).
pub fn staging_file(dir: &Path, tag: &str) -> Result<PathBuf, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    for _ in 0..16 {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".{tag}-{}-{nanos}-{n}.tmp", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(_) => return Ok(tmp),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{}: {e}", tmp.display())),
        }
    }
    Err(format!(
        "{}: could not create a staging file",
        dir.display()
    ))
}

pub fn load(dir: &Path, name: &str) -> Result<Settings, String> {
    let p = path(dir, name)?;
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let bad = |e: &dyn std::fmt::Display| format!("{}: {e}", p.display());
    let mut f: serde_json::Value = serde_json::from_str(&text).map_err(|e| bad(&e))?;
    let version = f.get("version").and_then(|v| v.as_u64()).unwrap_or(1);
    if version > VERSION as u64 {
        return Err(format!(
            "{} is from a newer Pix++ (profile version {version}).",
            p.display()
        ));
    }
    let mut settings = f
        .get_mut("settings")
        .map(serde_json::Value::take)
        .unwrap_or_else(|| serde_json::json!({}));
    if version < 2 {
        migrate_v1(&mut settings);
    }
    if version < 3 {
        migrate_v2(&mut settings);
    }
    serde_json::from_value(settings).map_err(|e| bad(&e))
}

pub fn delete(dir: &Path, name: &str) -> Result<(), String> {
    let p = path(dir, name)?;
    std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Method;
    use pixplusplus::pixeloe::color::Dither;
    use pixplusplus::pixeloe::downscale::Interp;
    use pixplusplus::pixeloe::pipeline::{Mode, Sharpen};
    use pixplusplus::Adjust;

    fn temp_dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("pixplusplus-profiles-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn round_trips_every_setting() {
        let dir = temp_dir("roundtrip");
        let mut s = Settings {
            method: Method::Pixeloe,
            size: 96,
            colors: 12,
            oe_quantise: true,
            importance: false,
            pia_saturation: 1.35,
            oe_saturation: 0.8,
            contrast: 1.2,
            pre: Adjust {
                saturation: 1.5,
                contrast: 0.9,
            },
            ..Settings::default()
        };
        s.oe.thickness = 2;
        s.oe.mode = Mode::Interp(Interp::Bicubic);
        s.oe.sharpen = Some(Sharpen::Laplacian);
        s.oe.dither = Dither::ErrorDiffusion;
        s.oe.post_match = true;
        save(&dir, "Frog test", &s).unwrap();
        assert_eq!(list(&dir), vec!["Frog test".to_string()]);
        assert_eq!(load(&dir, "Frog test").unwrap(), s);

        // Save replaces; delete removes.
        s.size = 64;
        save(&dir, "Frog test", &s).unwrap();
        assert_eq!(load(&dir, "Frog test").unwrap().size, 64);
        delete(&dir, "Frog test").unwrap();
        assert!(list(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let dir = temp_dir("partial");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("old.json"),
            r#"{"version": 1, "settings": {"size": 200, "oe": {"thickness": 1}}}"#,
        )
        .unwrap();
        let s = load(&dir, "old").unwrap();
        let d = Settings::default();
        assert_eq!(s.size, 200);
        assert_eq!(s.oe.thickness, 1);
        assert_eq!(
            (s.method, s.colors, s.contrast),
            (d.method, d.colors, d.contrast)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_pixeloe_options_keep_gui_defaults() {
        let dir = temp_dir("partial-oe");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("oe.json"),
            r#"{"version": 1, "settings": {"method": "pixeloe", "size": 48, "oe": {"thickness": 1}}}"#,
        )
        .unwrap();
        let s = load(&dir, "oe").unwrap();
        let d = Settings::default().oe;
        assert_eq!(s.oe.thickness, 1);
        assert_eq!(
            (s.oe.post_upscale, s.oe.post_match, s.oe.dither, s.oe.mode),
            (d.post_upscale, d.post_match, d.dither, d.mode)
        );
        // An explicit value is kept even when it equals the library default.
        std::fs::write(
            dir.join("oe2.json"),
            r#"{"version": 1, "settings": {"oe": {"dither": "ordered", "post_match": true}}}"#,
        )
        .unwrap();
        let s = load(&dir, "oe2").unwrap();
        assert_eq!((s.oe.dither, s.oe.post_match), (Dither::Ordered, true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_leave_no_staging_files() {
        let dir = temp_dir("staging");
        for i in 0..3 {
            save(
                &dir,
                "p",
                &Settings {
                    size: 40 + i,
                    ..Settings::default()
                },
            )
            .unwrap();
        }
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("p.json")]);
        assert_eq!(load(&dir, "p").unwrap().size, 42);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn older_profiles_without_a_palette_stay_native() {
        use crate::settings::PaletteChoice;
        let dir = temp_dir("v2-palette");
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body, want) in [
            (
                "v1",
                r#"{"version": 1, "settings": {}}"#,
                PaletteChoice::Native,
            ),
            (
                "v2",
                r#"{"version": 2, "settings": {"size": 64}}"#,
                PaletteChoice::Native,
            ),
            (
                "v2-set",
                r#"{"version": 2, "settings": {"palette": "decimate-oklab"}}"#,
                PaletteChoice::DecimateOklab,
            ),
            (
                "v3",
                r#"{"version": 3, "settings": {}}"#,
                PaletteChoice::Balanced,
            ),
        ] {
            std::fs::write(dir.join(format!("{name}.json")), body).unwrap();
            assert_eq!(load(&dir, name).unwrap().palette, want, "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_decimate_value_loads_as_rgb() {
        let dir = temp_dir("old-decimate");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("d.json"),
            r#"{"version": 1, "settings": {"palette": "decimate"}}"#,
        )
        .unwrap();
        assert_eq!(
            load(&dir, "d").unwrap().palette,
            crate::settings::PaletteChoice::DecimateRgb
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_1_profiles_keep_their_meaning() {
        let dir = temp_dir("v1");
        std::fs::create_dir_all(&dir).unwrap();
        let cases = [
            (
                r#"{"method": "pia", "importance": "auto"}"#,
                Method::Pia,
                false,
            ),
            (
                r#"{"method": "pia-importance", "importance": "auto"}"#,
                Method::Pia,
                true,
            ),
            (
                r#"{"method": "pia-importance", "importance": {"file": "/m.png"}}"#,
                Method::Pia,
                true,
            ),
            (r#"{"size": 64}"#, Method::Pia, false),
            (r#"{"method": "pixeloe"}"#, Method::Pixeloe, true),
        ];
        for (i, (settings, method, importance)) in cases.iter().enumerate() {
            let name = format!("v1-{i}");
            std::fs::write(
                dir.join(format!("{name}.json")),
                format!(r#"{{"version": 1, "settings": {settings}}}"#),
            )
            .unwrap();
            let s = load(&dir, &name).unwrap();
            assert_eq!(
                (s.method, s.importance),
                (*method, *importance),
                "{settings}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_unsafe_names() {
        for bad in [
            "",
            "  ",
            "../x",
            "a/b",
            "a\\b",
            ".hidden",
            "x.",
            "a:b",
            "a*b",
            &"n".repeat(65),
        ] {
            assert!(check_name(bad).is_err(), "{bad:?} accepted");
        }
        for good in ["Frog", "PixelOE 8 colours", "pia-k16_v2", "Été"] {
            assert_eq!(check_name(good), Ok(good));
        }
        assert_eq!(check_name("  padded  "), Ok("padded"));
    }

    #[test]
    fn rejects_newer_versions() {
        let dir = temp_dir("version");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("new.json"), r#"{"version": 99, "settings": {}}"#).unwrap();
        assert!(load(&dir, "new").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
