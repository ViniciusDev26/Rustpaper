//! Desktop adapters: everything that depends on the user's compositor.
//!
//! The engine's core (main.rs: WE control, Playback rules, watchdog, the
//! mirror) never talks to a compositor directly. It asks a [`Desktop`] which
//! monitors exist and what covers them, and a [`Stage`] where Wallpaper
//! Engine's windows can live out of sight. Supporting another desktop means
//! one new adapter implementing these traits; nothing else changes.
//!
//! What every adapter can rely on (standard Wayland protocols, used by
//! mirror.rs): `zwlr-layer-shell` to show the wallpaper, and
//! `ext-image-copy-capture` + `ext-foreign-toplevel-list` to capture WE's
//! windows.
//!
//! | Desktop | Adapter | Stage |
//! |---|---|---|
//! | Hyprland 0.56+ | [`hyprland`] | headless outputs + window rules |
//! | Sway 1.11+ | [`sway`] | headless outputs (`create_output`) + `for_window` |
//! | others (KDE, niri, ...) | none yet | a private stage (our own nested compositor) is the planned general answer, see docs/ENGINE.md |
//! | GNOME | none | no layer-shell: needs a Shell extension |

pub mod hyprland;
pub mod sway;

/// WE's window class under Proton (`steam_app_<AppID>`).
pub const WE_CLASS: &str = "steam_app_431960";

/// Title of the WE window that shows the wallpaper of monitor `output`.
/// Long-lived and shared by engine instances (see main.rs).
pub fn window_title(output: &str) -> String {
    format!("rustpaper-{output}")
}

/// A real monitor of the user's session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub name: String,
    /// Pixel size (WE renders 1:1; the mirror scales to the logical size).
    pub size: (u32, u32),
}

/// What covers one monitor: the conditions of the Playback settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Coverage {
    pub name: String,
    /// A fullscreen window is on its visible workspace.
    pub fullscreen: bool,
    /// A maximized or tiled window is on its visible workspace (tiling
    /// compositors leave the wallpaper visible only in the gaps).
    pub maximized: bool,
    /// The focused window is on it.
    pub focused: bool,
    /// Powered off (DPMS).
    pub asleep: bool,
}

/// The user's desktop session.
pub trait Desktop {
    /// For logs and errors.
    fn name(&self) -> &'static str;

    /// The real monitors (never the adapter's own hidden outputs).
    fn outputs(&self) -> Vec<Output>;

    /// What covers each monitor; None when the compositor cannot tell
    /// (the Playback rules then never pause).
    fn coverage(&self) -> Option<Vec<Coverage>>;

    /// Called before any layer surface exists (e.g. turn off the
    /// compositor's open/close animations for namespace `rustpaper`).
    fn prepare_layers(&self) {}

    /// Where WE's windows can live hidden; None when this desktop has no
    /// way to host them (yet).
    fn stage(&self) -> Option<&dyn Stage>;
}

/// A place where WE renders one window per monitor, out of the user's sight
/// but visible to the compositor (compositors only render, and so only let
/// us capture, windows they consider visible).
pub trait Stage {
    /// Creates (or reuses) the place for monitor `out`: a hidden output of
    /// its pixel size, and the rules that send WE's window
    /// `window_title(out.name)` there, fullscreen, without focus or
    /// animation. Idempotent.
    fn prepare(&self, out: &Output);

    /// Keeps WE's own windows (UI, dialogs) out of sight and unfocused.
    fn hide_we_ui(&self);

    /// Repairs `out`'s arrangement (window on its hidden output, fullscreen,
    /// shown). Returns whether WE's window exists at all.
    fn tend(&self, out: &Output) -> bool;
}

/// The adapter for the running session.
pub fn detect() -> Result<Box<dyn Desktop>, String> {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        return hyprland::Hyprland::connect().map(|d| Box::new(d) as Box<dyn Desktop>);
    }
    if std::env::var_os("SWAYSOCK").is_some() {
        return sway::Sway::connect().map(|d| Box::new(d) as Box<dyn Desktop>);
    }
    let session = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".into());
    Err(format!(
        "no desktop adapter for this session ({session}). Rustpaper supports Hyprland and \
         Sway; another desktop needs an adapter implementing desktop::Desktop and \
         desktop::Stage (see docs/ENGINE.md)"
    ))
}
