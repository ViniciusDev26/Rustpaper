// Module `steam`: Steam's per-user config, to set Wallpaper Engine's launch
// option without opening Steam or WE.
//
// Launch options live in `userdata/<account>/config/localconfig.vdf`
// (`UserLocalConfigStore > Software > Valve > Steam > apps > <appid> >
// LaunchOptions`), a text KeyValues ("VDF") file. Steam keeps it in memory
// and rewrites it when it exits, so it may only be edited while Steam is
// closed; `steam_running()` tells.

use std::path::{Path, PathBuf};

/// A KeyValues node: a string, or a section of ordered children.
#[derive(Debug, Clone, PartialEq)]
pub enum Vdf {
    Str(String),
    Section(Vec<(String, Vdf)>),
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

/// Parses a text KeyValues document (the top-level children).
pub fn parse(text: &str) -> Result<Vec<(String, Vdf)>, String> {
    let mut tokens = Vec::new();
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        match c {
            '"' => {
                let mut raw = String::new();
                let mut closed = false;
                while let Some((_, d)) = it.next() {
                    match d {
                        '\\' => {
                            raw.push('\\');
                            if let Some((_, e)) = it.next() {
                                raw.push(e);
                            }
                        }
                        '"' => {
                            closed = true;
                            break;
                        }
                        _ => raw.push(d),
                    }
                }
                if !closed {
                    return Err(format!("unterminated string at byte {i}"));
                }
                tokens.push(Some(unescape(&raw)));
            }
            '{' => tokens.push(None),
            '}' => tokens.push(Some("\u{0}}".into())),
            '/' if matches!(it.peek(), Some((_, '/'))) => {
                for (_, d) in it.by_ref() {
                    if d == '\n' {
                        break;
                    }
                }
            }
            c if c.is_whitespace() => {}
            other => return Err(format!("unexpected {other:?} at byte {i}")),
        }
    }
    let mut pos = 0;
    let doc = section(&tokens, &mut pos, true)?;
    Ok(doc)
}

fn section(
    tokens: &[Option<String>],
    pos: &mut usize,
    top: bool,
) -> Result<Vec<(String, Vdf)>, String> {
    let mut out = Vec::new();
    while *pos < tokens.len() {
        match &tokens[*pos] {
            Some(t) if t == "\u{0}}" => {
                if top {
                    return Err("unbalanced '}'".into());
                }
                *pos += 1;
                return Ok(out);
            }
            Some(key) => {
                *pos += 1;
                match tokens.get(*pos) {
                    Some(None) => {
                        *pos += 1;
                        out.push((key.clone(), Vdf::Section(section(tokens, pos, false)?)));
                    }
                    Some(Some(v)) if v != "\u{0}}" => {
                        *pos += 1;
                        out.push((key.clone(), Vdf::Str(v.clone())));
                    }
                    _ => return Err(format!("key {key:?} has no value")),
                }
            }
            None => return Err("'{' without a key".into()),
        }
    }
    if top {
        Ok(out)
    } else {
        Err("missing '}'".into())
    }
}

/// Serializes in Steam's own layout (tabs, quoted keys and values).
pub fn write(doc: &[(String, Vdf)]) -> String {
    fn go(out: &mut String, items: &[(String, Vdf)], depth: usize) {
        let ind = "\t".repeat(depth);
        for (k, v) in items {
            match v {
                Vdf::Str(s) => {
                    out.push_str(&format!("{ind}\"{}\"\t\t\"{}\"\n", escape(k), escape(s)))
                }
                Vdf::Section(c) => {
                    out.push_str(&format!("{ind}\"{}\"\n{ind}{{\n", escape(k)));
                    go(out, c, depth + 1);
                    out.push_str(&format!("{ind}}}\n"));
                }
            }
        }
    }
    let mut out = String::new();
    go(&mut out, doc, 0);
    out
}

/// The child section `key` (case-insensitive, like Steam), created when
/// missing.
fn child<'a>(items: &'a mut Vec<(String, Vdf)>, key: &str) -> &'a mut Vec<(String, Vdf)> {
    let idx = match items
        .iter()
        .position(|(k, v)| k.eq_ignore_ascii_case(key) && matches!(v, Vdf::Section(_)))
    {
        Some(i) => i,
        None => {
            items.push((key.to_string(), Vdf::Section(Vec::new())));
            items.len() - 1
        }
    };
    match &mut items[idx].1 {
        Vdf::Section(c) => c,
        Vdf::Str(_) => unreachable!("matched a section"),
    }
}

const APPS_PATH: [&str; 5] = ["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"];

/// `LaunchOptions` of `appid` in a localconfig document.
pub fn get_launch_options(doc: &[(String, Vdf)], appid: &str) -> Option<String> {
    let mut items = doc;
    for key in APPS_PATH.iter().copied().chain(std::iter::once(appid)) {
        items = items.iter().find_map(|(k, v)| match v {
            Vdf::Section(c) if k.eq_ignore_ascii_case(key) => Some(c.as_slice()),
            _ => None,
        })?;
    }
    items.iter().find_map(|(k, v)| match v {
        Vdf::Str(s) if k.eq_ignore_ascii_case("LaunchOptions") => Some(s.clone()),
        _ => None,
    })
}

/// Sets (or, with an empty string, clears) `LaunchOptions` of `appid`.
pub fn set_launch_options(doc: &mut Vec<(String, Vdf)>, appid: &str, options: &str) {
    let mut items = doc;
    for key in APPS_PATH {
        items = child(items, key);
    }
    let app = child(items, appid);
    app.retain(|(k, _)| !k.eq_ignore_ascii_case("LaunchOptions"));
    if !options.is_empty() {
        app.push(("LaunchOptions".into(), Vdf::Str(options.into())));
    }
}

const COMPAT_PATH: [&str; 5] = [
    "InstallConfigStore",
    "Software",
    "Valve",
    "Steam",
    "CompatToolMapping",
];

/// The compatibility tool forced for `appid` (Steam's per-game "Force the
/// use of a specific Steam Play compatibility tool"), in a config.vdf
/// document. None: Steam's default for the game.
pub fn get_compat_tool(doc: &[(String, Vdf)], appid: &str) -> Option<String> {
    let mut items = doc;
    for key in COMPAT_PATH.iter().copied().chain(std::iter::once(appid)) {
        items = items.iter().find_map(|(k, v)| match v {
            Vdf::Section(c) if k.eq_ignore_ascii_case(key) => Some(c.as_slice()),
            _ => None,
        })?;
    }
    items
        .iter()
        .find_map(|(k, v)| match v {
            Vdf::Str(s) if k.eq_ignore_ascii_case("name") => Some(s.clone()),
            _ => None,
        })
        .filter(|s| !s.is_empty())
}

/// Forces compatibility tool `tool` (its internal name) for `appid`, or with
/// None goes back to Steam's default.
pub fn set_compat_tool(doc: &mut Vec<(String, Vdf)>, appid: &str, tool: Option<&str>) {
    let mut items = doc;
    for key in COMPAT_PATH {
        items = child(items, key);
    }
    items.retain(|(k, _)| k != appid);
    if let Some(tool) = tool {
        // what Steam writes for a per-game choice
        items.push((
            appid.to_string(),
            Vdf::Section(vec![
                ("name".into(), Vdf::Str(tool.into())),
                ("config".into(), Vdf::Str(String::new())),
                ("priority".into(), Vdf::Str("250".into())),
            ]),
        ));
    }
}

/// Steam's `config/config.vdf` (where per-game compatibility tools live).
pub fn steam_config() -> Option<PathBuf> {
    Some(crate::paths::steam_root()?.join("config/config.vdf")).filter(|p| p.is_file())
}

/// The compatibility tool forced for `appid` in `file`.
pub fn read_compat_tool(file: &Path, appid: &str) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    get_compat_tool(&parse(&text).ok()?, appid)
}

/// Writes `appid`'s compatibility tool into `file` (one-time backup next to
/// it). Steam must be closed: it rewrites the file on exit.
pub fn write_compat_tool(file: &Path, appid: &str, tool: Option<&str>) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut doc = parse(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    set_compat_tool(&mut doc, appid, tool);
    let backup = file.with_extension("vdf.rustpaper-backup");
    if !backup.exists() {
        std::fs::copy(file, &backup).map_err(|e| format!("{}: {e}", backup.display()))?;
    }
    std::fs::write(file, write(&doc)).map_err(|e| format!("{}: {e}", file.display()))
}

/// Every `userdata/<account>/config/localconfig.vdf` of the Steam install.
pub fn local_configs() -> Vec<PathBuf> {
    let Some(root) = crate::paths::steam_root() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(root.join("userdata")) else {
        return Vec::new();
    };
    rd.flatten()
        .map(|e| e.path().join("config/localconfig.vdf"))
        .filter(|p| p.is_file())
        .collect()
}

/// WE's launch options in `file`.
pub fn read_launch_options(file: &Path, appid: &str) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    get_launch_options(&parse(&text).ok()?, appid)
}

/// Writes WE's launch options into `file` (one-time backup next to it).
/// Steam must be closed: it rewrites the file on exit.
pub fn write_launch_options(file: &Path, appid: &str, options: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut doc = parse(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    set_launch_options(&mut doc, appid, options);
    let backup = file.with_extension("vdf.rustpaper-backup");
    if !backup.exists() {
        std::fs::copy(file, &backup).map_err(|e| format!("{}: {e}", backup.display()))?;
    }
    std::fs::write(file, write(&doc)).map_err(|e| format!("{}: {e}", file.display()))
}

/// The Steam client is running (its main process is named `steam`).
pub fn steam_running() -> bool {
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return false;
    };
    rd.flatten().any(|e| {
        std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim_end() == "steam")
    })
}

#[cfg(test)]
mod tests {

    #[test]
    fn compat_tool_is_set_read_and_cleared() {
        let mut doc = parse("\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"cip\"\t\t\"x\"\n\t\t\t}\n\t\t}\n\t}\n}\n").unwrap();
        assert_eq!(get_compat_tool(&doc, "431960"), None);
        set_compat_tool(&mut doc, "431960", Some("GE-Proton11-13-x86_64"));
        let text = write(&doc);
        let back = parse(&text).unwrap();
        assert_eq!(
            get_compat_tool(&back, "431960").as_deref(),
            Some("GE-Proton11-13-x86_64")
        );
        assert!(text.contains("\"cip\""), "other keys kept");
        let mut back = back;
        set_compat_tool(&mut back, "431960", None);
        assert_eq!(get_compat_tool(&back, "431960"), None);
    }

    use super::*;

    const SAMPLE: &str = "\"UserLocalConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"apps\"\n\t\t\t\t{\n\t\t\t\t\t\"431960\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"LastPlayed\"\t\t\"1795384941\"\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n\t\"friends\"\n\t{\n\t\t\"name\"\t\t\"a \\\"quoted\\\" name\"\n\t}\n}\n";

    #[test]
    fn round_trips_steam_layout() {
        let doc = parse(SAMPLE).unwrap();
        assert_eq!(
            write(&doc),
            SAMPLE,
            "Steam's own layout comes back byte for byte"
        );
    }

    #[test]
    fn sets_and_reads_launch_options() {
        let mut doc = parse(SAMPLE).unwrap();
        assert_eq!(get_launch_options(&doc, "431960"), None);
        set_launch_options(&mut doc, "431960", "/usr/bin/rustpaper we-launch %command%");
        let text = write(&doc);
        let doc = parse(&text).unwrap();
        assert_eq!(
            get_launch_options(&doc, "431960").as_deref(),
            Some("/usr/bin/rustpaper we-launch %command%")
        );
        assert!(
            text.contains("\"LastPlayed\"\t\t\"1795384941\""),
            "other keys kept"
        );
        let mut doc = doc;
        set_launch_options(&mut doc, "431960", "");
        assert_eq!(get_launch_options(&doc, "431960"), None);
    }

    #[test]
    fn creates_missing_sections() {
        let mut doc = parse("\"UserLocalConfigStore\"\n{\n}\n").unwrap();
        set_launch_options(&mut doc, "431960", "x %command%");
        assert_eq!(
            get_launch_options(&doc, "431960").as_deref(),
            Some("x %command%")
        );
    }

    /// The user's real localconfig.vdf parses and is rewritten unchanged
    /// (read only; skipped when there is none).
    #[test]
    fn real_localconfig_round_trips() {
        for file in local_configs() {
            let text = std::fs::read_to_string(&file).unwrap();
            let doc = parse(&text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
            assert_eq!(write(&doc), text, "{}", file.display());
        }
    }

    #[test]
    fn rejects_broken_files() {
        assert!(parse("\"a\"\n{\n").is_err());
        assert!(parse("\"a\" \"b").is_err());
        assert!(parse("}").is_err());
    }
}
