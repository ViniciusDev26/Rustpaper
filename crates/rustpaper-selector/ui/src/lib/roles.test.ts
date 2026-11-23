import { describe, expect, it } from "vitest";
import type { RolesView } from "../api/schemas";
import { parsePlacementKey, placementKey, placementsOf, removable, targets } from "./roles";

const roles = (over: Partial<RolesView> = {}): RolesView => ({
  outputs: [
    { name: "HDMI-A-1", width: 3840, height: 2160 },
    { name: "DP-1", width: 1920, height: 1080 },
  ],
  desktop: "a",
  monitors: { "DP-1": "b" },
  lock: "c",
  screensaver: { enabled: true, seconds: 150, all: "a", monitors: { "HDMI-A-1": "b" } },
  ...over,
});

describe("wallpaper roles", () => {
  it("lists every target, per monitor only with several monitors", () => {
    expect(targets(roles()).map((t) => t.label)).toEqual([
      "Desktop",
      "Desktop · HDMI-A-1",
      "Desktop · DP-1",
      "Lock screen",
      "Screensaver",
      "Screensaver · HDMI-A-1",
      "Screensaver · DP-1",
    ]);
    const one = roles({ outputs: [{ name: "eDP-1", width: 1920, height: 1200 }] });
    expect(targets(one).map((t) => t.label)).toEqual(["Desktop", "Lock screen", "Screensaver"]);
  });

  it("finds where a wallpaper was put", () => {
    expect(placementsOf("a", roles()).map((p) => p.label)).toEqual(["Desktop", "Screensaver"]);
    expect(placementsOf("b", roles()).map((p) => p.label)).toEqual(["Desktop · DP-1", "Screensaver · HDMI-A-1"]);
    expect(placementsOf("c", roles()).map((p) => p.label)).toEqual(["Lock screen"]);
    expect(placementsOf("z", roles())).toEqual([]);
  });

  it("round-trips select keys and keeps the base desktop", () => {
    expect(parsePlacementKey(placementKey({ role: "screensaver", output: "DP-1" }))).toEqual({
      role: "screensaver",
      output: "DP-1",
    });
    expect(parsePlacementKey("lock:")).toEqual({ role: "lock", output: null });
    expect(parsePlacementKey("evil:x")).toBeNull();
    expect(removable({ role: "desktop", output: null })).toBe(false);
    expect(removable({ role: "desktop", output: "DP-1" })).toBe(true);
  });
});
