//! `rustpaper we-proton enable|disable|status [--json]`: runs Wallpaper
//! Engine on Wine's Wayland driver, an opt-in for less CPU.
//!
//! Steam's own Proton builds ship only Wine's X11 driver. There WE renders
//! into a child window, which Wine always composites offscreen, and the
//! stage's XWayland copies every frame again (docs/ENGINE.md). GE-Proton
//! also ships Wine's Wayland driver: WE's window becomes an xdg_toplevel of
//! the stage and its Vulkan swapchain a subsurface, with no XWayland work.
//! Measured on the real session (4K + 1080p sharing one window, RTX 4060
//! Ti, 2026-11-23): CPU 15.8% -> 5.4% of a core, the card 37.8 W -> 33.3 W,
//! the whole GPU unchanged (35.8% -> 35.0%: NVIDIA's Wayland Vulkan path
//! does the copy XWayland did). Again with 0.7.0 installed: CPU 17.1% ->
//! 4.7%, the card 33.0 W -> 31.7 W.
//!
//! `enable` uses the newest GE-Proton in Steam's `compatibilitytools.d`, or
//! downloads the latest release (its published SHA-512 checked), forces it
//! as WE's compatibility tool in Steam's config.vdf (Steam closed and
//! started again, like `setup-steam`), and sets `wine_wayland` in
//! state.json; the launch option then starts WE with the Wayland driver
//! (launch.rs). `disable` gives WE back Steam's default tool.

use std::path::{Path, PathBuf};
use std::process::Command;

use rustpaper_core::{paths, state, steam};

const RELEASES: &str =
    "https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases/latest";

pub fn main(args: &[&str]) -> i32 {
    let result = match args.first().copied() {
        Some("enable") => enable(),
        Some("disable") => disable(),
        Some("status") => {
            status(args.contains(&"--json"));
            Ok(())
        }
        _ => Err("usage: rustpaper we-proton enable|disable|status [--json]".into()),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("we-proton: {e}");
            1
        }
    }
}

fn tools_dir() -> Option<PathBuf> {
    Some(paths::steam_root()?.join("compatibilitytools.d"))
}

/// `GE-Proton11-13` (`-x86_64` or not) -> (11, 13).
fn ge_version(name: &str) -> Option<(u32, u32)> {
    let rest = name.strip_prefix("GE-Proton")?;
    let rest = rest.strip_suffix("-x86_64").unwrap_or(rest);
    let (major, minor) = rest.split_once('-')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// The internal name a tool folder registers (its compatibilitytool.vdf).
fn internal_name(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("compatibilitytool.vdf")).ok()?;
    let doc = steam::parse(&text).ok()?;
    let tools = doc.iter().find_map(|(k, v)| match v {
        steam::Vdf::Section(c) if k.eq_ignore_ascii_case("compatibilitytools") => Some(c),
        _ => None,
    })?;
    let list = tools.iter().find_map(|(k, v)| match v {
        steam::Vdf::Section(c) if k.eq_ignore_ascii_case("compat_tools") => Some(c),
        _ => None,
    })?;
    list.first().map(|(k, _)| k.clone())
}

/// The installed GE-Proton builds that carry Wine's Wayland driver, newest
/// first: (internal name, folder).
fn installed() -> Vec<(String, PathBuf)> {
    let Some(dir) = tools_dir() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("files/lib/wine/x86_64-unix/winewayland.so").exists())
        .filter_map(|p| Some((internal_name(&p)?, p)))
        .filter(|(n, _)| ge_version(n).is_some())
        .collect();
    out.sort_by_key(|(n, _)| std::cmp::Reverse(ge_version(n)));
    out
}

fn run(cmd: &mut Command) -> Result<Vec<u8>, String> {
    let out = cmd
        .output()
        .map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if !out.status.success() {
        return Err(format!(
            "{:?} failed: {}",
            cmd.get_program(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

/// Downloads the latest GE-Proton release into compatibilitytools.d, after
/// checking its published SHA-512. Returns its internal name.
fn download_latest() -> Result<String, String> {
    let json = run(Command::new("curl").args(["-sfL", RELEASES]))?;
    let release: serde_json::Value = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
    let assets = release["assets"]
        .as_array()
        .ok_or("no assets in the release")?;
    let url_of = |pred: &dyn Fn(&str) -> bool| {
        assets.iter().find_map(|a| {
            let name = a["name"].as_str()?;
            let url = a["browser_download_url"].as_str()?;
            pred(name).then(|| (name.to_string(), url.to_string()))
        })
    };
    let (tar_name, tar_url) = url_of(&|n| n.ends_with(".tar.gz") && !n.contains("aarch64"))
        .ok_or("no x86_64 tarball in the release")?;
    let (_, sum_url) = url_of(&|n| n.ends_with(".sha512sum") && !n.contains("aarch64"))
        .ok_or("no checksum in the release")?;
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or("no cache directory")?
        .join("rustpaper");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let tarball = cache.join(&tar_name);
    println!("downloading {tar_name} (about 600 MB)...");
    run(Command::new("curl")
        .args(["-sfL", "-o"])
        .arg(&tarball)
        .arg(&tar_url))?;
    let expected = String::from_utf8_lossy(&run(Command::new("curl").args(["-sfL", &sum_url]))?)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    let actual = String::from_utf8_lossy(&run(Command::new("sha512sum").arg(&tarball))?)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    if expected.is_empty() || expected != actual {
        let _ = std::fs::remove_file(&tarball);
        return Err(format!("{tar_name}: checksum mismatch, not installed"));
    }
    let dir = tools_dir().ok_or("Steam not found")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    run(Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(&dir))?;
    let _ = std::fs::remove_file(&tarball);
    installed()
        .into_iter()
        .next()
        .map(|(n, _)| n)
        .ok_or_else(|| format!("{tar_name} did not install a GE-Proton with Wine's Wayland driver"))
}

/// Forces `tool` (None: Steam's default) for WE, with Steam closed.
fn set_tool(tool: Option<&str>) -> Result<(), String> {
    let file = steam::steam_config().ok_or("Steam's config/config.vdf not found")?;
    let app = paths::WE_APP_ID;
    if steam::read_compat_tool(&file, app).as_deref() == tool {
        return Ok(());
    }
    crate::launch::with_steam_closed(|| {
        steam::write_compat_tool(&file, app, tool)?;
        println!(
            "Wallpaper Engine's compatibility tool: {}",
            tool.unwrap_or("Steam's default")
        );
        Ok(())
    })
}

fn save(enabled: bool) -> Result<(), String> {
    let mut st = state::load();
    st.wine_wayland = enabled;
    state::save(&st).map_err(|e| e.to_string())?;
    // a running engine restarts WE with the new driver (main.rs reload)
    let _ = Command::new("systemctl")
        .args(["--user", "reload", "rustpaper.service"])
        .status();
    Ok(())
}

fn enable() -> Result<(), String> {
    let tool = match installed().into_iter().next() {
        Some((name, _)) => name,
        None => download_latest()?,
    };
    set_tool(Some(&tool))?;
    save(true)?;
    // the first start on GE-Proton updates WE's Wine prefix: ~2 min, 2026-11-23
    println!(
        "Wallpaper Engine restarts on Wine's Wayland driver ({tool}); the wallpaper pauses until \
         it is back (about 2 minutes the first time, under a minute after)."
    );
    Ok(())
}

fn disable() -> Result<(), String> {
    let current = steam::steam_config().and_then(|f| steam::read_compat_tool(&f, paths::WE_APP_ID));
    // only undo our own choice: a tool the user forced stays
    if current.as_deref().is_some_and(|t| ge_version(t).is_some()) {
        set_tool(None)?;
    }
    save(false)?;
    println!(
        "Wallpaper Engine restarts on Wine's X11 driver; the wallpaper pauses until it is back \
         (under a minute)."
    );
    Ok(())
}

fn status(json: bool) {
    let enabled = state::load().wine_wayland;
    let tool = steam::steam_config().and_then(|f| steam::read_compat_tool(&f, paths::WE_APP_ID));
    let installed: Vec<String> = installed().into_iter().map(|(n, _)| n).collect();
    if json {
        println!(
            "{}",
            serde_json::json!({ "enabled": enabled, "tool": tool, "installed": installed })
        );
    } else {
        println!(
            "Wine Wayland driver: {} (WE's tool: {}; installed: {})",
            if enabled { "on" } else { "off" },
            tool.as_deref().unwrap_or("Steam's default"),
            if installed.is_empty() {
                "none".to_string()
            } else {
                installed.join(", ")
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ge_versions_order_releases() {
        assert_eq!(ge_version("GE-Proton11-13-x86_64"), Some((11, 13)));
        assert_eq!(ge_version("GE-Proton9-27"), Some((9, 27)));
        assert_eq!(ge_version("proton_experimental"), None);
        assert!(ge_version("GE-Proton11-2") < ge_version("GE-Proton11-13"));
    }
}
