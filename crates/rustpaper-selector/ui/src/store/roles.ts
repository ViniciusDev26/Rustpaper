// Where each wallpaper shows (desktop per monitor, lock screen, screensaver)
// and the screensaver's settings. Every change is saved by the backend and
// the running engine reloads it.
import { create } from "zustand";
import type { Role, RolesView } from "../api/schemas";
import { api } from "../api/tauri";
import { placementLabel } from "../lib/roles";
import { errorText, toast } from "./toast";

interface RolesState {
  roles: RolesView | null;
  load: () => Promise<void>;
  assign: (id: string, title: string, role: Role, output: string | null) => Promise<void>;
  clear: (role: Role, output: string | null) => Promise<void>;
  setScreensaver: (enabled: boolean, seconds: number) => Promise<void>;
}

export const useRoles = create<RolesState>((set, get) => ({
  roles: null,
  load: async () => {
    try {
      set({ roles: await api.getRoles() });
    } catch (e) {
      toast(errorText(e));
    }
  },
  assign: async (id, title, role, output) => {
    try {
      await api.apply(id, role, output);
      toast(`${placementLabel(role, output)}: ${title || id}`);
    } catch (e) {
      toast(`Error: ${errorText(e)}`);
    }
    await get().load();
  },
  clear: async (role, output) => {
    try {
      await api.clearRole(role, output);
      toast(`${placementLabel(role, output)}: back to the default`);
    } catch (e) {
      toast(`Error: ${errorText(e)}`);
    }
    await get().load();
  },
  setScreensaver: async (enabled, seconds) => {
    try {
      await api.setScreensaver(enabled, seconds);
      toast(enabled ? `Screensaver after ${Math.round(seconds / 6) / 10} min` : "Screensaver off");
    } catch (e) {
      toast(`Error: ${errorText(e)}`);
    }
    await get().load();
  },
}));
