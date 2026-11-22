// Typed wrappers around the selector's Tauri commands. Every reply is
// validated with its zod schema; command errors (Rust `Err(String)`) reject
// with that string.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { z } from "zod";
import {
  DesktopInfo,
  MaybeTheme,
  PropertiesView,
  Settings,
  StatusMap,
  Subscriptions,
  Wallpapers,
  WorkshopPage,
  type WorkshopQuery,
} from "./schemas";

async function call<S extends z.ZodTypeAny>(
  schema: S,
  cmd: string,
  args?: Record<string, unknown>,
): Promise<z.infer<S>> {
  let raw: unknown;
  try {
    raw = await invoke(cmd, args);
  } catch (e) {
    throw new Error(typeof e === "string" ? e : String(e));
  }
  const parsed = schema.safeParse(raw);
  if (!parsed.success) {
    console.error(`${cmd}: unexpected reply`, parsed.error.issues, raw);
    throw new Error(`${cmd}: unexpected reply from the backend`);
  }
  return parsed.data;
}

async function run(cmd: string, args?: Record<string, unknown>): Promise<void> {
  try {
    await invoke(cmd, args);
  } catch (e) {
    throw new Error(typeof e === "string" ? e : String(e));
  }
}

/// Local file (preview image) -> URL the webview can load.
export const fileUrl = (path: string) => convertFileSrc(path);

export const api = {
  listWallpapers: () => call(Wallpapers, "list_wallpapers"),
  apply: (id: string) => run("apply", { id }),
  getProperties: (id: string) => call(PropertiesView, "get_properties", { id }),
  saveProperties: (id: string, values: Record<string, unknown>) =>
    run("save_properties", { id, values }),
  getSettings: () => call(Settings, "get_settings"),
  setAutostart: (enabled: boolean) => run("set_autostart", { enabled }),
  setScripts: (enabled: boolean) => run("set_scripts", { enabled }),
  setVolume: (volume: number) => run("set_volume", { volume }),
  workshopQuery: (query: WorkshopQuery) => call(WorkshopPage, "workshop_query", { query }),
  workshopSubscribe: (id: string) => run("workshop_subscribe", { id }),
  workshopUnsubscribe: (id: string) => run("workshop_unsubscribe", { id }),
  workshopStatus: (ids: string[]) => call(StatusMap, "workshop_status", { ids }),
  workshopSubscriptions: () => call(Subscriptions, "workshop_subscriptions"),
  getTheme: () => call(MaybeTheme, "get_theme"),
  desktopInfo: () => call(DesktopInfo, "desktop_info"),
  themeFromWallpaper: (id: string) => run("theme_from_wallpaper", { id }),
  setWallpaperTheme: (enabled: boolean) => run("set_wallpaper_theme", { enabled }),
};

/// Result of the background theme generation after an apply: null = done,
/// a string = the error.
export async function onWallpaperTheme(cb: (error: string | null) => void): Promise<() => void> {
  const { listen } = await import("@tauri-apps/api/event");
  return listen<unknown>("wallpaper-theme", (e) => cb(typeof e.payload === "string" ? e.payload : null));
}

/// Omarchy theme switches (emitted by the backend's watcher).
export async function onThemeChanged(cb: (t: z.infer<typeof MaybeTheme>) => void): Promise<() => void> {
  const { listen } = await import("@tauri-apps/api/event");
  return listen<unknown>("theme-changed", (e) => {
    const parsed = MaybeTheme.safeParse(e.payload);
    if (parsed.success) cb(parsed.data);
  });
}
