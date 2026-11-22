// Engine-wide settings in the header: autostart, scripts, master volume.
// Each change is saved by the backend, which restarts the running wallpaper
// when the setting is on its command line.
import { create } from "zustand";
import { api } from "../api/tauri";
import { errorText, toast } from "./toast";

interface SettingsState {
  loaded: boolean;
  autostart: boolean;
  scripts: boolean;
  volume: number;
  lastId: string | null;
  /// Omarchy theme from each applied wallpaper (needs Omarchy)
  wallpaperTheme: boolean;
  omarchy: boolean;
  load: () => Promise<void>;
  setWallpaperTheme: (on: boolean) => Promise<void>;
  setAutostart: (on: boolean) => Promise<void>;
  setScripts: (on: boolean) => Promise<void>;
  setVolume: (volume: number) => Promise<void>;
}

export const volumeText = (v: number) => (v === 0 ? "Muted" : `Volume ${v}%`);

export const useSettings = create<SettingsState>((set, get) => ({
  loaded: false,
  autostart: false,
  scripts: true,
  volume: 100,
  lastId: null,
  wallpaperTheme: false,
  omarchy: false,
  load: async () => {
    const [s, desktop] = await Promise.all([api.getSettings(), api.desktopInfo()]);
    set({
      loaded: true,
      autostart: s.autostart,
      scripts: !s.no_scripts,
      volume: s.volume,
      lastId: s.last_id,
      wallpaperTheme: s.wallpaper_theme,
      omarchy: desktop.omarchy,
    });
  },
  setWallpaperTheme: async (on) => {
    const before = get().wallpaperTheme;
    set({ wallpaperTheme: on });
    if (on) toast("Theming the desktop from the wallpaper…");
    try {
      await api.setWallpaperTheme(on);
      toast(on ? "Desktop theme follows the wallpaper" : "Desktop theme left as is");
    } catch (e) {
      set({ wallpaperTheme: before });
      toast(errorText(e));
    }
  },
  setAutostart: async (on) => {
    const before = get().autostart;
    set({ autostart: on });
    try {
      await api.setAutostart(on);
      toast(on ? "Autostart on" : "Autostart off");
    } catch (e) {
      set({ autostart: before });
      toast(errorText(e));
    }
  },
  setScripts: async (on) => {
    const before = get().scripts;
    set({ scripts: on });
    try {
      await api.setScripts(on);
      toast(on ? "Scripts on" : "Scripts off");
    } catch (e) {
      set({ scripts: before });
      toast(errorText(e));
    }
  },
  /// Called on release of the slider (each call restarts the wallpaper).
  setVolume: async (volume) => {
    set({ volume });
    try {
      await api.setVolume(volume);
      toast(volumeText(volume));
    } catch (e) {
      toast(errorText(e));
    }
  },
}));
