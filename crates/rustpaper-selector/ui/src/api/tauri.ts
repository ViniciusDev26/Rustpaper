// Typed wrappers around the selector's Tauri commands. Every reply is
// validated with its zod schema; command errors (Rust `Err(String)`) reject
// with that string.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { z } from "zod";
import {
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
};
