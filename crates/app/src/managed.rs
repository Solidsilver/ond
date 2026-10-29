//! Settings set outside the app, in `managed.json` beside the settings file
//! (the Home Manager module writes it). They lie over the user's own settings
//! from start-up on, edits can't change them, and Settings shows their
//! controls locked. The user's own values for them stay in the settings file,
//! so a setting taken out of the managed file goes back to what it was.
//!
//! A few settings are adopted instead: when the managed value differs, it
//! replaces the user's own, because changing it has to do something (sign in
//! to the new server, move the downloads).

use crate::settings::Settings;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// What a managed file may set, as paths into the settings file. A path
/// ending in `*` takes any key there. The rest of the settings file is the
/// app's own state (where windows were, the volume, column widths).
const ALLOWED: &[&str] = &[
    "server",
    "username",
    "scrobble",
    "diagnostics_bar",
    "window.system_title_bar",
    "theme",
    "accent",
    "adapt_colors",
    "backdrop",
    "waveform",
    "waveform_bars.rounded",
    "waveform_bars.width",
    "waveform_bars.gap",
    "alpha_rail",
    "artist_photos",
    "playback.replay_gain",
    "playback.preamp_db",
    "playback.untagged_db",
    "playback.quality",
    "playback.format",
    "playback.kbps",
    "queue.play_next",
    "queue.add",
    "queue.on_play",
    "queue.clear",
    "output_device",
    "device_modes.*",
    "output_match_rate",
    "device_volume_green",
    "device_volume_db",
    "device_change_toast",
    "device_hidden",
    "device_groups.*",
    "stream_cache_mb",
    "offline_dir",
    "offline_limit_gb",
    "analyze_playing",
    "experimental.spectrum",
    "experimental.spectrogram",
    "experimental.visualizer",
    "experimental.visualizer_pick",
];

/// Settings that replace the user's own when they differ (see the module docs).
const ADOPTED: &[&str] = &["server", "username", "offline_dir"];

/// Shown beside a setting's locked control.
pub const LOCKED: &str = "Set in your Nix config";

/// The file's name, beside the settings file.
pub fn path_beside(config_file: &Path) -> PathBuf {
    config_file.with_file_name("managed.json")
}

/// The settings a managed file sets, each checked against the settings it
/// lies over.
#[derive(Debug, Default, Clone)]
pub struct Managed {
    /// Each setting as its path and value.
    leaves: Vec<(Vec<String>, Value)>,
}

impl Managed {
    /// Reads the file beside `config_file`. Missing is none managed; a file
    /// that can't be read, or a setting that isn't allowed, is logged and left out.
    pub fn read(config_file: &Path) -> Self {
        let path = path_beside(config_file);
        let Ok(bytes) = std::fs::read(&path) else { return Self::default() };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(top)) => Self::from_object(&top),
            Ok(_) => {
                tracing::warn!("{}: not a JSON object; ignored", path.display());
                Self::default()
            }
            Err(e) => {
                tracing::warn!("{}: {e}; ignored", path.display());
                Self::default()
            }
        }
    }

    /// Reads the file beside `config_file` and puts its settings into `s`
    /// (the window's first size and frame, before the app starts).
    pub fn lay_over(config_file: &Path, s: &mut Settings) {
        let mut m = Self::read(config_file);
        if m.is_empty() {
            return;
        }
        m.check(&serde_json::to_value(&*s).unwrap_or_default());
        m.apply(s);
    }

    fn from_object(top: &Map<String, Value>) -> Self {
        let mut leaves = Vec::new();
        collect(top, &mut Vec::new(), &mut leaves);
        Self { leaves }
    }

    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    /// Keeps only the settings that read as settings over `base` (the user's
    /// own, as JSON), so one wrong value can't make the whole file unreadable.
    pub fn check(&mut self, base: &Value) {
        let mut v = base.clone();
        self.leaves.retain(|(path, value)| {
            let mut tried = v.clone();
            set(&mut tried, path, value.clone());
            match serde_json::from_value::<Settings>(tried.clone()) {
                Ok(_) => {
                    v = tried;
                    true
                }
                Err(e) => {
                    tracing::warn!("managed setting {}: {e}; ignored", path.join("."));
                    false
                }
            }
        });
    }

    /// Puts the managed values into `s`.
    pub fn apply(&self, s: &mut Settings) {
        if self.leaves.is_empty() {
            return;
        }
        let Ok(mut v) = serde_json::to_value(&*s) else { return };
        for (path, value) in &self.leaves {
            set(&mut v, path, value.clone());
        }
        match serde_json::from_value(v) {
            Ok(managed) => *s = managed,
            Err(e) => tracing::warn!("applying managed settings: {e}"),
        }
    }

    /// Adopts the settings in [`ADOPTED`] into `users` (the user's own, as
    /// JSON) where they differ. Returns each changed one's path and old value.
    pub fn adopt(&self, users: &mut Value) -> Vec<(String, Value)> {
        let mut changed = Vec::new();
        for (path, value) in &self.leaves {
            let key = path.join(".");
            if !ADOPTED.contains(&key.as_str()) {
                continue;
            }
            let old = get(users, path).cloned().unwrap_or(Value::Null);
            if old != *value {
                set(users, path, value.clone());
                changed.push((key, old));
            }
        }
        changed
    }

    /// In `saved` (settings about to be written), puts back the user's own
    /// value from `users` for each setting that lies over it, or leaves it out
    /// when they had none.
    pub fn restore(&self, saved: &mut Value, users: &Value) {
        for (path, _) in &self.leaves {
            if ADOPTED.contains(&path.join(".").as_str()) {
                continue;
            }
            match get(users, path) {
                Some(own) => set(saved, path, own.clone()),
                None => remove(saved, path),
            }
        }
    }

    /// Whether `path` (dotted, like `playback.kbps`) is managed, wholly or in part.
    pub fn has(&self, path: &str) -> bool {
        let want: Vec<&str> = path.split('.').collect();
        self.leaves.iter().any(|(p, _)| p.iter().zip(&want).all(|(a, b)| a == b))
    }
}

/// Walks `obj`, collecting the allowed settings' paths and values and
/// logging the rest.
fn collect(obj: &Map<String, Value>, at: &mut Vec<String>, out: &mut Vec<(Vec<String>, Value)>) {
    for (key, value) in obj {
        at.push(key.clone());
        let dotted = at.join(".");
        let exact = ALLOWED.contains(&dotted.as_str());
        let under = ALLOWED.iter().any(|a| a.strip_suffix('*').is_some_and(|prefix| dotted.starts_with(prefix)) || a.starts_with(&format!("{dotted}.")));
        let any_key = ALLOWED.contains(&format!("{dotted}.*").as_str());
        match value {
            // Like a folder picked in Settings, one of Ond's own inside it.
            Value::String(dir) if dotted == "offline_dir" => out.push((at.clone(), Value::String(crate::settings::offline_folder(dir.into()).to_string_lossy().into_owned()))),
            _ if exact => out.push((at.clone(), value.clone())),
            Value::Object(inner) if under || any_key => collect(inner, at, out),
            _ if under && at.len() > 1 => out.push((at.clone(), value.clone())),
            _ => tracing::warn!("managed setting {dotted}: not one Ond takes from managed.json; ignored"),
        }
        at.pop();
    }
}

fn get<'a>(v: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(v, |v, k| v.get(k))
}

/// Sets `path` in `v`, making the objects on the way.
fn set(v: &mut Value, path: &[String], value: Value) {
    let Some((last, parents)) = path.split_last() else { return };
    let mut at = v;
    for k in parents {
        if !at.get(k).is_some_and(Value::is_object)
            && let Some(o) = at.as_object_mut()
        {
            o.insert(k.clone(), Value::Object(Map::new()));
        }
        let Some(next) = at.get_mut(k) else { return };
        at = next;
    }
    if let Some(o) = at.as_object_mut() {
        o.insert(last.clone(), value);
    }
}

fn remove(v: &mut Value, path: &[String]) {
    let Some((last, parents)) = path.split_last() else { return };
    let Some(parent) = parents.iter().try_fold(v, |v, k| v.get_mut(k)) else { return };
    if let Some(o) = parent.as_object_mut() {
        o.remove(last);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemePref;
    use serde_json::json;

    fn managed(v: Value) -> Managed {
        let Value::Object(top) = v else { panic!() };
        Managed::from_object(&top)
    }

    #[test]
    fn allowed_settings_are_taken_and_app_state_isnt() {
        let m = managed(json!({
            "theme": "dark",
            "playback": { "replay_gain": "album", "kbps": 192 },
            "queue": { "on_play": "ask" },
            "device_modes": { "uid-umc22": "bit-perfect" },
            "window": { "system_title_bar": true, "mode": "mini" },
            "volume": 0.5,
            "sidebar_width": 300.0,
        }));
        assert!(m.has("theme") && m.has("playback.replay_gain") && m.has("playback.kbps") && m.has("device_modes.uid-umc22") && m.has("window.system_title_bar"));
        assert!(m.has("queue.on_play") && !m.has("queue.add"), "queue settings are managed one by one");
        assert!(m.has("playback"), "part of it");
        assert!(!m.has("playback.quality") && !m.has("window.mode") && !m.has("volume") && !m.has("sidebar_width") && !m.has("device_modes.other"));
    }

    #[test]
    fn managed_settings_lie_over_the_users_own() {
        let mut m = managed(json!({ "theme": "dark", "playback": { "kbps": 192 }, "experimental": { "visualizer": true } }));
        let own = Settings { theme: ThemePref::Light, scrobble: false, ..Default::default() };
        m.check(&serde_json::to_value(&own).unwrap());
        let mut s = own.clone();
        m.apply(&mut s);
        assert_eq!(s.theme, ThemePref::Dark);
        assert_eq!(s.playback.kbps, 192);
        assert!(s.experimental.visualizer);
        assert!(!s.scrobble, "what isn't managed stays the user's");
        assert_eq!(s.playback.replay_gain, own.playback.replay_gain, "the rest of a group too");
    }

    #[test]
    fn a_wrong_value_is_left_out_and_the_rest_still_apply() {
        let mut m = managed(json!({ "theme": "purple", "scrobble": "yes", "alpha_rail": true }));
        m.check(&serde_json::to_value(Settings::default()).unwrap());
        assert!(!m.has("theme") && !m.has("scrobble"));
        let mut s = Settings::default();
        m.apply(&mut s);
        assert!(s.alpha_rail);
    }

    #[test]
    fn saving_keeps_the_users_own_values_for_managed_settings() {
        let m = managed(json!({ "theme": "dark", "alpha_rail": true, "playback": { "kbps": 192 } }));
        let users = json!({ "server": "", "username": "", "theme": "light", "playback": { "kbps": 320 } });
        let mut s = Settings::default();
        m.apply(&mut s);
        let mut saved = serde_json::to_value(&s).unwrap();
        m.restore(&mut saved, &users);
        assert_eq!(saved["theme"], "light");
        assert_eq!(saved["playback"]["kbps"], 320);
        assert!(saved.get("alpha_rail").is_none(), "the user never set it");
    }

    #[test]
    fn the_server_and_downloads_folder_are_adopted() {
        let m = managed(json!({ "server": "https://music.example.com", "username": "me", "offline_dir": "/data/music", "theme": "dark" }));
        let mut users = json!({ "server": "https://old.example.com", "username": "me", "theme": "light" });
        let changed = m.adopt(&mut users);
        assert_eq!(changed, vec![("offline_dir".into(), Value::Null), ("server".into(), json!("https://old.example.com"))]);
        assert_eq!(managed(json!({ "offline_dir": "/data/Ond Offline" })).leaves[0].1, "/data/Ond Offline");
        assert_eq!(users["server"], "https://music.example.com");
        assert_eq!(users["theme"], "light", "the rest only lie over it");
        // Saved, adopted settings stay as they now are.
        let mut saved = users.clone();
        m.restore(&mut saved, &users);
        let dir = saved["offline_dir"].as_str().unwrap();
        assert_eq!(Path::new(dir), Path::new("/data/music/Ond Offline"), "a folder of Ond's own inside it, as when picked in Settings");
        assert!(m.adopt(&mut users).is_empty(), "nothing changes the second time");
    }
}
