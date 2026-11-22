// Shapes of what the Rust commands return (src/main.rs, src/workshop.rs and
// crates/rustpaper-workshop/src/protocol.rs), checked at the boundary so a
// backend change surfaces as one clear error instead of undefined fields
// deep in the UI.
import { z } from "zod";

export const WallpaperKind = z.enum(["scene", "video", "web", "unknown"]);
export type WallpaperKind = z.infer<typeof WallpaperKind>;

/// list_wallpapers -> WallpaperEntry (main.rs)
export const Wallpaper = z.object({
  id: z.string(),
  title: z.string(),
  kind: WallpaperKind,
  preview: z.string().nullable(),
  supported: z.boolean(),
  workshop: z.boolean(),
  added: z.number(),
});
export type Wallpaper = z.infer<typeof Wallpaper>;
export const Wallpapers = z.array(Wallpaper);

/// get_settings -> settings::Settings
export const Settings = z.object({
  last_id: z.string().nullable(),
  autostart: z.boolean(),
  no_scripts: z.boolean(),
  volume: z.number().int().min(0).max(100),
});
export type Settings = z.infer<typeof Settings>;

/// One `general.properties` entry of project.json, as written by Workshop
/// authors: loosely typed on purpose (values are numbers, strings or bools
/// depending on the property and the author).
export const PropertyValue = z.union([z.string(), z.number(), z.boolean(), z.null()]);
export type PropertyValue = z.infer<typeof PropertyValue>;

export const Property = z
  .object({
    name: z.string(),
    type: z.string().optional(),
    text: z.string().nullish(),
    value: z.unknown().optional(),
    order: z.number().optional(),
    condition: z.string().optional(),
    min: z.coerce.number().optional(),
    max: z.coerce.number().optional(),
    step: z.coerce.number().optional(),
    precision: z.coerce.number().optional(),
    fraction: z.boolean().optional(),
    options: z
      .array(z.object({ label: z.unknown().optional(), value: z.unknown() }).passthrough())
      .optional(),
  })
  .passthrough();
export type Property = z.infer<typeof Property>;

/// get_properties -> PropertiesView
export const PropertiesView = z.object({
  properties: z.array(Property),
  saved: z.record(z.unknown()),
  applied: z.boolean(),
});
export type PropertiesView = z.infer<typeof PropertiesView>;

// ---- Workshop (rustpaper-workshop protocol.rs) ----------------------------

export const ItemState = z.object({
  subscribed: z.boolean(),
  installed: z.boolean(),
  downloading: z.boolean(),
  needs_update: z.boolean(),
  downloaded: z.number(),
  total: z.number(),
});
export type ItemState = z.infer<typeof ItemState>;

export const WorkshopItem = z.object({
  id: z.string(),
  title: z.string(),
  preview_url: z.string(),
  kind: z.enum(["scene", "video", "web"]),
  rating: z.enum(["everyone", "questionable", "mature"]),
  tags: z.array(z.string()),
  subscriptions: z.number(),
  votes_up: z.number(),
  votes_down: z.number(),
  file_size: z.number(),
  time_updated: z.number(),
  state: ItemState,
});
export type WorkshopItem = z.infer<typeof WorkshopItem>;

export const WorkshopPage = z.object({
  total: z.number(),
  page: z.number(),
  items: z.array(WorkshopItem),
});
export type WorkshopPage = z.infer<typeof WorkshopPage>;

export const StatusMap = z.record(ItemState);
export const Subscriptions = z.record(z.number());

export const WorkshopSort = z.enum(["trend", "top", "subscriptions", "recent", "updated", "subscribed"]);
export type WorkshopSort = z.infer<typeof WorkshopSort>;
export const WorkshopKind = z.enum(["all", "scene", "video", "web"]);
export type WorkshopKind = z.infer<typeof WorkshopKind>;

export interface WorkshopQuery {
  sort: WorkshopSort;
  kind: WorkshopKind;
  search: string;
  mature: boolean;
  page: number;
}
