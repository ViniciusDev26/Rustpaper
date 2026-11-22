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
  load: () => Promise<void>;
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
  load: async () => {
    const s = await api.getSettings();
    set({ loaded: true, autostart: s.autostart, scripts: !s.no_scripts, volume: s.volume, lastId: s.last_id });
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
