// Where each wallpaper shows: the desktop (every monitor, or one), the lock
// screen, the screensaver (every monitor, or one). Pure helpers over the
// backend's RolesView (main.rs get_roles), for the "Use for" menu and the
// badges; state.json's fallbacks are the backend's business.
import type { Role, RolesView } from "../api/schemas";

export interface Placement {
  role: Role;
  /// null: every monitor (or the lock screen, which has one wallpaper)
  output: string | null;
  label: string;
}

const NAME: Record<Role, string> = { desktop: "Desktop", lock: "Lock screen", screensaver: "Screensaver" };

export function placementLabel(role: Role, output: string | null): string {
  return output ? `${NAME[role]} · ${output}` : NAME[role];
}

/// `role:output` for a <select> value, and back.
export const placementKey = (p: Pick<Placement, "role" | "output">) => `${p.role}:${p.output ?? ""}`;
export function parsePlacementKey(key: string): Pick<Placement, "role" | "output"> | null {
  const [role, output] = key.split(":", 2);
  if (role !== "desktop" && role !== "lock" && role !== "screensaver") return null;
  return { role, output: output || null };
}

/// Every place a wallpaper can be put. Per-monitor entries only with more
/// than one monitor.
export function targets(r: RolesView): Placement[] {
  const per = r.outputs.length > 1 ? r.outputs.map((o) => o.name) : [];
  const out: Placement[] = [{ role: "desktop", output: null, label: placementLabel("desktop", null) }];
  for (const o of per) out.push({ role: "desktop", output: o, label: placementLabel("desktop", o) });
  out.push({ role: "lock", output: null, label: placementLabel("lock", null) });
  out.push({ role: "screensaver", output: null, label: placementLabel("screensaver", null) });
  for (const o of per) out.push({ role: "screensaver", output: o, label: placementLabel("screensaver", o) });
  return out;
}

/// Where wallpaper `id` was put (explicit choices, not fallbacks).
export function placementsOf(id: string, r: RolesView): Placement[] {
  const out: Placement[] = [];
  const add = (role: Role, output: string | null) => out.push({ role, output, label: placementLabel(role, output) });
  if (r.desktop === id) add("desktop", null);
  for (const [o, v] of Object.entries(r.monitors)) if (v === id) add("desktop", o);
  if (r.lock === id) add("lock", null);
  if (r.screensaver.all === id) add("screensaver", null);
  for (const [o, v] of Object.entries(r.screensaver.monitors)) if (v === id) add("screensaver", o);
  return out;
}

/// The desktop for every monitor cannot be removed: everything falls back
/// to it.
export const removable = (p: Pick<Placement, "role" | "output">) => !(p.role === "desktop" && p.output === null);
