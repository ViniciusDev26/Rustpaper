// The Performance tab, like Wallpaper Engine's own: quality settings (WE's
// presets and each setting; the engine restarts WE with them) and Playback
// rules (what the engine pauses on), and what plays video wallpapers (mpv or
// Wallpaper Engine). Drawn from the data the backend sends
// (rustpaper-core performance.rs): a new WE setting needs no UI code.
import { useEffect, useRef, useState } from "react";
import type {
  PerformanceView as View,
  Playback,
  PlaybackAction,
  QualitySetting,
  SettingValue,
  VideoPlayer,
} from "../api/schemas";
import { api } from "../api/tauri";
import { useRoles } from "../store/roles";
import { errorText, toast } from "../store/toast";
import { Button, field } from "./common";

const PRESET_LABEL: Record<string, string> = { low: "Low", medium: "Medium", high: "High", ultra: "Ultra", custom: "Custom" };

/// A quality change restarts Wallpaper Engine: one restart per burst of edits.
const SAVE_DELAY_MS = 900;

function Section({ title, note, children }: { title: string; note: string; children: React.ReactNode }) {
  return (
    <section className="mb-7">
      <h2 className="mb-1 text-xs uppercase tracking-widest text-accent">{title}</h2>
      <p className="mb-3 text-[0.78rem] text-muted">{note}</p>
      {children}
    </section>
  );
}

function SettingRow({ label, hint, children }: { label: string; hint: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[minmax(160px,220px)_1fr] items-center gap-x-4 gap-y-0.5 border-b border-line py-2.5">
      <div className="text-sm">{label}</div>
      <div>{children}</div>
      <div />
      <div className="text-[0.72rem] text-muted">{hint}</div>
    </div>
  );
}

function QualityControl({ s, value, set }: { s: QualitySetting; value: SettingValue; set: (v: SettingValue) => void }) {
  switch (s.kind) {
    case "select":
      return (
        <select className={`${field} min-w-44`} value={String(value)} onChange={(e) => set(e.currentTarget.value)}>
          {s.options.map(([v, label]) => (
            <option key={v} value={v}>
              {label}
            </option>
          ))}
        </select>
      );
    case "slider": {
      const n = Number(value);
      return (
        <div className="flex items-center gap-3">
          <input
            type="range"
            className="w-56"
            min={s.min}
            max={s.max}
            step={s.step}
            value={n}
            onChange={(e) => set(Number(e.currentTarget.value))}
          />
          <span className="w-10 tabular-nums text-sub">{n}</span>
          {s.key === "fps" && n > 30 && <span className="text-[0.72rem] text-yellow">High FPS costs a lot of GPU</span>}
        </div>
      );
    }
    case "toggle":
      return (
        <label className="flex cursor-pointer items-center gap-2 text-sm text-sub">
          <input type="checkbox" checked={!!value} onChange={(e) => set(e.currentTarget.checked)} />
          {value ? "On" : "Off"}
        </label>
      );
  }
}

/// The preset these values match (what WE's UI shows), else "custom".
function presetOf(view: View, values: Record<string, SettingValue>): string {
  const match = view.presets.find((p) => Object.entries(p.values).every(([k, v]) => values[k] === v));
  return match?.name ?? "custom";
}

/// The screensaver switch and its delay (which wallpaper each monitor
/// shows is chosen in the Library: Apply to → Screensaver).
function ScreensaverSection() {
  const { roles, load, setScreensaver } = useRoles();
  const [minutes, setMinutes] = useState<string | null>(null);
  useEffect(() => {
    if (!roles) void load();
  }, [roles, load]);
  if (!roles) return null;
  const s = roles.screensaver;
  const shown = minutes ?? String(Math.round(s.seconds / 6) / 10);
  const commit = (enabled: boolean, text: string) => {
    const m = Number(text.replace(",", "."));
    const secs = Number.isFinite(m) && m > 0 ? Math.round(m * 60) : s.seconds;
    setMinutes(null);
    void setScreensaver(enabled, secs);
  };
  return (
    <Section
      title="Screensaver"
      note="After this long without input, each monitor shows its screensaver wallpaper over your windows; any key or the mouse ends it. Pick the wallpapers in the Library (Apply to → Screensaver); without one, a monitor shows its desktop wallpaper. While on, it replaces Omarchy's screensaver."
    >
      <SettingRow label="Screensaver" hint="Idle inhibitors (a playing video) hold it off.">
        <div className="flex flex-wrap items-center gap-3">
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" checked={s.enabled} onChange={(e) => commit(e.currentTarget.checked, shown)} />
            On
          </label>
          <span className="text-sm text-sub">after</span>
          <input
            className={`${field} w-20`}
            inputMode="decimal"
            value={shown}
            onChange={(e) => setMinutes(e.currentTarget.value)}
            onBlur={() => minutes !== null && commit(s.enabled, minutes)}
            onKeyDown={(e) => e.key === "Enter" && commit(s.enabled, shown)}
          />
          <span className="text-sm text-sub">minutes</span>
        </div>
      </SettingRow>
    </Section>
  );
}

/// Opt-in: WE on Wine's Wayland driver (GE-Proton). Applying it closes and
/// reopens Steam, may download GE-Proton (about 600 MB) and restarts WE.
function WineWaylandSection({ initial }: { initial: boolean }) {
  const [on, setOn] = useState(initial);
  const [busy, setBusy] = useState(false);
  const change = async (next: boolean) => {
    setBusy(true);
    toast(
      next
        ? "Setting up GE-Proton (Steam restarts; the first time it may download ~600 MB)…"
        : "Back to Steam's Proton (Steam restarts)…",
    );
    try {
      const note = await api.setWineWayland(next);
      setOn(next);
      toast(note || (next ? "Wine's Wayland driver on" : "Wine's X11 driver back"));
    } catch (e) {
      toast(`Error: ${errorText(e)}`);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Section
      title="Wine's Wayland driver (advanced)"
      note="Runs Wallpaper Engine on GE-Proton with Wine's Wayland driver, so no XWayland copies its frames: about a quarter of the CPU (17.1% → 4.7% of a core, 4K + 1080p) and a little less power. On NVIDIA the GPU load stays the same. Applying it closes and reopens Steam, and the wallpaper pauses until Wallpaper Engine is back: about 2 minutes the first time (Steam updates its Wine prefix; GE-Proton may also need a ~600 MB download), under a minute after."
    >
      <SettingRow label="Wayland driver" hint="Off gives Wallpaper Engine back Steam's own Proton.">
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={on} disabled={busy} onChange={(e) => void change(e.currentTarget.checked)} />
          {busy ? "Applying…" : on ? "On (GE-Proton)" : "Off"}
        </label>
      </SettingRow>
    </Section>
  );
}

export function PerformanceView() {
  const [view, setView] = useState<View | null>(null);
  const [values, setValues] = useState<Record<string, SettingValue>>({});
  const [playback, setPlaybackState] = useState<Playback | null>(null);
  const [videoPlayer, setVideoPlayerState] = useState<VideoPlayer>("we");
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    api.getPerformance().then(
      (v) => {
        setView(v);
        setValues(v.values);
        setPlaybackState(v.playback);
        setVideoPlayerState(v.video_player);
      },
      (e) => setError(errorText(e)),
    );
  }, []);
  useEffect(() => () => clearTimeout(timer.current), []);

  if (error) return <div className="p-5 text-sub">Failed to load: {error}</div>;
  if (!view || !playback) return <div className="p-5 text-sub">Loading…</div>;

  const saveQuality = (next: Record<string, SettingValue>) => {
    setValues(next);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      api.setPerformance(next).then(
        () => toast("Quality saved: Wallpaper Engine restarts with it"),
        (e) => toast(`Error: ${errorText(e)}`),
      );
    }, SAVE_DELAY_MS);
  };

  const setAction = (key: keyof Playback, action: PlaybackAction) => {
    const next = { ...playback, [key]: action };
    setPlaybackState(next);
    api.setPlayback(next).then(
      () => toast("Playback rules saved"),
      (e) => toast(`Error: ${errorText(e)}`),
    );
  };

  const setVideoPlayer = (player: VideoPlayer) => {
    setVideoPlayerState(player);
    api.setVideoPlayer(player).then(
      () => toast(player === "mpv" ? "Video wallpapers play in mpv" : "Video wallpapers play in Wallpaper Engine"),
      (e) => toast(`Error: ${errorText(e)}`),
    );
  };

  const preset = presetOf(view, values);
  return (
    <div className="flex-1 overflow-y-auto">
      <div className="mx-auto max-w-3xl p-6">
        <Section
          title="Quality"
          note="Wallpaper Engine's own quality settings. Changing them restarts Wallpaper Engine (a few seconds without wallpaper)."
        >
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <span className="mr-1 text-sm text-sub">Preset</span>
            {view.presets.map((p) => (
              <Button
                key={p.name}
                variant={preset === p.name ? "primary" : "secondary"}
                onClick={() => saveQuality({ ...values, ...p.values })}
              >
                {PRESET_LABEL[p.name] ?? p.name}
              </Button>
            ))}
            {preset === "custom" && <span className="text-[0.78rem] text-muted">Custom</span>}
          </div>
          {view.settings.map((s) => (
            <SettingRow key={s.key} label={s.label} hint={s.hint}>
              <QualityControl s={s} value={values[s.key] ?? s.default} set={(v) => saveQuality({ ...values, [s.key]: v })} />
            </SettingRow>
          ))}
        </Section>
        <Section
          title="Video wallpapers"
          note="Wallpaper Engine decodes videos on the CPU under Proton; mpv uses the GPU's video decoder for the same picture at a fraction of the cost. Scenes and web wallpapers always play in Wallpaper Engine."
        >
          <SettingRow
            label="Play videos with"
            hint={
              view.mpv_installed
                ? "mpv needs the private stage (rustpaper setup-steam); otherwise Wallpaper Engine plays them."
                : "mpv is not installed: install it (e.g. pacman -S mpv) to use it. Until then Wallpaper Engine plays videos."
            }
          >
            <select
              className={`${field} min-w-44`}
              value={videoPlayer}
              onChange={(e) => setVideoPlayer(e.currentTarget.value as VideoPlayer)}
            >
              <option value="mpv">mpv (hardware decoding)</option>
              <option value="we">Wallpaper Engine</option>
            </select>
          </SettingRow>
        </Section>
        <WineWaylandSection initial={view.wine_wayland} />
        <ScreensaverSection />
        <Section
          title="Playback"
          note="When to pause the wallpaper on a monitor. Paused, it costs nothing; Wallpaper Engine itself pauses when every monitor is paused."
        >
          {view.playback_settings.map((s) => (
            <SettingRow key={s.key} label={s.label} hint={s.hint}>
              <select
                className={`${field} min-w-44`}
                value={playback[s.key]}
                onChange={(e) => setAction(s.key, e.currentTarget.value as PlaybackAction)}
              >
                <option value="run">Keep running</option>
                <option value="pause">Pause</option>
              </select>
            </SettingRow>
          ))}
        </Section>
      </div>
    </div>
  );
}
