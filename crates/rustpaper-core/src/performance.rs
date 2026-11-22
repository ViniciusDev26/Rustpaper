// Module `performance`: Wallpaper Engine's quality settings, as data.
//
// The selector's Performance tab is drawn from this table and the engine
// writes the chosen values into WE's `config.json` (`<user>.general.user`).
// Keys, values and presets are WE's own, read from its UI bundle
// (`ui/dist/scripts/scripts.js`: `textureResolutionOptions`,
// `postProcessingOptions`, `lightingOptions`, `fpsSlider`, `getQualityPreset`;
// WE 2.8, 2026-11). Adding a setting is one entry here.

use serde::Serialize;
use serde_json::{Map, Value, json};

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Control {
    /// One of fixed values.
    Select {
        options: Vec<(&'static str, &'static str)>,
    },
    /// An integer range.
    Slider {
        min: i64,
        max: i64,
        step: i64,
    },
    Toggle,
}

#[derive(Serialize, Clone, Debug)]
pub struct Setting {
    /// Key in WE's config.json.
    pub key: &'static str,
    pub label: &'static str,
    #[serde(flatten)]
    pub control: Control,
    /// WE's default (its "medium" preset).
    pub default: Value,
    /// Shown under the control.
    pub hint: &'static str,
}

const LIGHTING: [(&str, &str); 5] = [
    ("disabled", "Disabled"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("ultra", "Ultra"),
];

/// WE's quality settings, in the order of its Performance tab.
pub fn settings() -> Vec<Setting> {
    vec![
        Setting {
            key: "fps",
            label: "FPS",
            // WE's slider goes 10..50; higher values work but WE warns
            control: Control::Slider {
                min: 10,
                max: 60,
                step: 1,
            },
            default: json!(15),
            hint: "Frames per second WE renders. The biggest cost: 30 costs about half of 60.",
        },
        Setting {
            key: "msaa",
            label: "Anti-aliasing",
            control: Control::Select {
                options: vec![
                    ("none", "None"),
                    ("x2", "MSAA x2"),
                    ("x4", "MSAA x4"),
                    ("x8", "MSAA x8"),
                ],
            },
            default: json!("none"),
            hint: "Smoother edges in 3D scenes. x8 is for high-end graphics cards.",
        },
        Setting {
            key: "resolution",
            label: "Texture resolution",
            control: Control::Select {
                options: vec![
                    ("full", "High quality"),
                    ("half", "High performance"),
                    ("auto", "Automatic"),
                ],
            },
            default: json!("full"),
            hint: "High performance loads textures at half size: less video memory.",
        },
        Setting {
            key: "postprocessing",
            label: "Post-processing",
            control: Control::Select {
                options: vec![
                    ("disabled", "Disabled"),
                    ("enabled", "Enabled"),
                    ("ultra", "Ultra"),
                ],
            },
            default: json!("enabled"),
            hint: "Bloom and other screen effects. Ultra adds HDR bloom where supported.",
        },
        Setting {
            key: "reflection",
            label: "Reflections",
            control: Control::Toggle,
            default: json!(true),
            hint: "Reflections in scenes that use them.",
        },
        Setting {
            key: "shadows",
            label: "Shadows",
            control: Control::Select {
                options: LIGHTING.to_vec(),
            },
            default: json!("medium"),
            hint: "Shadow quality in 3D scenes.",
        },
        Setting {
            key: "volumetrics",
            label: "Volumetric lighting",
            control: Control::Select {
                options: LIGHTING.to_vec(),
            },
            default: json!("medium"),
            hint: "Light shafts in scenes that use them.",
        },
    ]
}

/// WE's quality presets (`getQualityPreset`): name -> values.
pub fn presets() -> Vec<(&'static str, Map<String, Value>)> {
    let p = |msaa, fps, post, refl, light| {
        let mut m = Map::new();
        m.insert("msaa".into(), json!(msaa));
        m.insert("resolution".into(), json!("full"));
        m.insert("fps".into(), json!(fps));
        m.insert("postprocessing".into(), json!(post));
        m.insert("reflection".into(), json!(refl));
        m.insert("shadows".into(), json!(light));
        m.insert("volumetrics".into(), json!(light));
        m
    };
    vec![
        ("low", p("none", 10, "disabled", false, "low")),
        ("medium", p("none", 15, "enabled", true, "medium")),
        ("high", p("x2", 25, "enabled", true, "high")),
        ("ultra", p("x2", 30, "ultra", true, "ultra")),
    ]
}

/// Keeps only known keys with values a setting accepts (the state file is
/// user-editable; WE is not fed anything else). `preset` is derived.
pub fn sanitize(values: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for s in settings() {
        let Some(v) = values.get(s.key) else { continue };
        let ok = match &s.control {
            Control::Select { options } => v
                .as_str()
                .is_some_and(|x| options.iter().any(|(o, _)| *o == x)),
            Control::Slider { min, max, .. } => {
                v.as_i64().is_some_and(|x| (*min..=*max).contains(&x))
            }
            Control::Toggle => v.is_boolean(),
        };
        if ok {
            out.insert(s.key.into(), v.clone());
        }
    }
    if !out.is_empty() {
        out.insert("preset".into(), json!(preset_of(&out)));
    }
    out
}

/// The values to show: the user's saved choice, else what WE's config.json
/// has, else WE's defaults; for every setting.
pub fn current(saved: &Map<String, Value>) -> Map<String, Value> {
    let saved = sanitize(saved);
    let we = sanitize(&we_config_values());
    settings()
        .into_iter()
        .map(|s| {
            let v = saved
                .get(s.key)
                .or_else(|| we.get(s.key))
                .cloned()
                .unwrap_or(s.default);
            (s.key.to_string(), v)
        })
        .collect()
}

/// The quality keys of WE's config.json (`<user>.general.user`), first user.
fn we_config_values() -> Map<String, Value> {
    let Some(path) = crate::paths::we_dir().map(|d| d.join("config.json")) else {
        return Map::new();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok())
        .and_then(|j| {
            j.as_object()?
                .values()
                .find_map(|u| u.pointer("/general/user")?.as_object().cloned())
        })
        .unwrap_or_default()
}

/// One Playback condition (WE's Playback settings), as data for the UI; its
/// value lives in `state::Playback` under `key`.
#[derive(Serialize, Clone, Debug)]
pub struct PlaybackSetting {
    pub key: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
}

pub fn playback_settings() -> Vec<PlaybackSetting> {
    vec![
        PlaybackSetting {
            key: "focused",
            label: "Other application focused",
            hint: "Any window on the monitor has the keyboard focus.",
        },
        PlaybackSetting {
            key: "maximized",
            label: "Other application maximized or tiled",
            hint: "A maximized window, or tiled windows, cover the monitor: the wallpaper only shows in the gaps.",
        },
        PlaybackSetting {
            key: "fullscreen",
            label: "Other application fullscreen",
            hint: "A fullscreen window (a game, a video) covers the monitor.",
        },
        PlaybackSetting {
            key: "sleep",
            label: "Display asleep",
            hint: "The monitor is powered off.",
        },
        PlaybackSetting {
            key: "battery",
            label: "Laptop on battery",
            hint: "No power adapter is plugged in.",
        },
    ]
}

/// The preset these values match, or "custom" (what WE's UI shows).
pub fn preset_of(values: &Map<String, Value>) -> &'static str {
    presets()
        .into_iter()
        .find(|(_, p)| p.iter().all(|(k, v)| values.get(k) == Some(v)))
        .map(|(name, _)| name)
        .unwrap_or("custom")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_only_use_known_values() {
        for (name, values) in presets() {
            let clean = sanitize(&values);
            assert_eq!(
                clean.len(),
                values.len() + 1,
                "{name}: every value accepted (+ preset)"
            );
            assert_eq!(preset_of(&values), name);
        }
    }

    #[test]
    fn current_has_every_setting() {
        let saved = json!({ "fps": 24 });
        let cur = current(saved.as_object().unwrap());
        assert_eq!(cur.get("fps"), Some(&json!(24)), "the saved choice wins");
        assert_eq!(cur.len(), settings().len());
    }

    #[test]
    fn playback_keys_exist_in_state() {
        let p = serde_json::to_value(crate::state::Playback::default()).unwrap();
        for s in playback_settings() {
            assert!(p.get(s.key).is_some(), "{} is a Playback field", s.key);
        }
    }

    #[test]
    fn sanitize_drops_unknown_keys_and_values() {
        let v = json!({ "fps": 30, "msaa": "x16", "safemode": true, "reflection": false });
        let clean = sanitize(v.as_object().unwrap());
        assert_eq!(clean.get("fps"), Some(&json!(30)));
        assert!(clean.get("msaa").is_none(), "x16 is not a WE value");
        assert!(clean.get("safemode").is_none(), "not a quality setting");
        assert_eq!(clean.get("preset"), Some(&json!("custom")));
    }
}
