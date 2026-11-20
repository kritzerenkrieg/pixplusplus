//! Named settings profiles (L1): one JSON file per profile in the platform config folder.
//!
//! File shape: `{"version": 1, "settings": {...}}`. Missing settings take the defaults
//! (`Settings` is `#[serde(default)]`), so profiles written by older builds keep loading.

use crate::settings::Settings;
use std::path::{Path, PathBuf};

/// Profile file format version.
const VERSION: u32 = 1;
/// Longest profile name, in characters.
const MAX_NAME: usize = 64;

#[derive(serde::Serialize, serde::Deserialize)]
struct File {
    version: u32,
    settings: Settings,
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
        settings: settings.clone(),
    })
    .map_err(|e| e.to_string())?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, json + "\n")
        .and_then(|()| std::fs::rename(&tmp, &p))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("{}: {e}", p.display())
        })
}

pub fn load(dir: &Path, name: &str) -> Result<Settings, String> {
    let p = path(dir, name)?;
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let f: File = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))?;
    if f.version > VERSION {
        return Err(format!(
            "{} is from a newer Pix++ (profile version {}).",
            p.display(),
            f.version
        ));
    }
    Ok(f.settings)
}

pub fn delete(dir: &Path, name: &str) -> Result<(), String> {
    let p = path(dir, name)?;
    std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{ImportanceSource, Method};
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
            method: Method::PiaImportance,
            size: 96,
            colors: 12,
            oe_quantise: true,
            importance: ImportanceSource::File(PathBuf::from("/maps/frog.png")),
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
