// Engine-wide settings in the header: start on login (the engine's systemd
// user service), master volume, theme from wallpaper. Each change is saved
// by the backend; the running engine reloads it in place.
import { create } from "zustand";
import { api } from "../api/tauri";
import { errorText, toast } from "./toast";

interface SettingsState {
  loaded: boolean;
  autostart: boolean;
  volume: number;
  lastId: string | null;
  /// Omarchy theme from each applied wallpaper (needs Omarchy)
  wallpaperTheme: boolean;
  omarchy: boolean;
  load: () => Promise<void>;
  setWallpaperTheme: (on: boolean) => Promise<void>;
  setAutostart: (on: boolean) => Promise<void>;
  setVolume: (volume: number) => Promise<void>;
}

export const volumeText = (v: number) => (v === 0 ? "Muted" : `Volume ${v}%`);

export const useSettings = create<SettingsState>((set, get) => ({
  loaded: false,
  autostart: false,
  volume: 100,
  lastId: null,
  wallpaperTheme: false,
  omarchy: false,
  load: async () => {
    const [s, desktop] = await Promise.all([api.getSettings(), api.desktopInfo()]);
    set({
      loaded: true,
      autostart: s.autostart,
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
      toast(on ? "Starts on login (rustpaper.service enabled)" : "Won't start on login");
    } catch (e) {
      set({ autostart: before });
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
