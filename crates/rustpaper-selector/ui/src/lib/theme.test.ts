import { describe, expect, it } from "vitest";
import { themeVars } from "./theme";

describe("Omarchy theme mapping", () => {
  it("maps the semantic palette", () => {
    const v = themeVars({
      name: "wet-girl",
      mode: "dark",
      colors: {
        background: "#1e1e2e",
        dark_background: "#171723",
        darker_background: "#0f0f17",
        lighter_background: "#353543",
        foreground: "#cdd6f4",
        dark_foreground: "#9aa1b7",
        accent: "#89b4fa",
        muted: "#45475a",
        red: "#f38ba8",
        magenta: "#cba6f7",
      },
    });
    expect(v.bg).toBe("#1e1e2e");
    expect(v.bar).toBe("#171723");
    expect(v.card).toBe("#353543");
    expect(v.accent).toBe("#89b4fa");
    expect(v.sub).toBe("#9aa1b7");
    expect(v.pink).toBe("#cba6f7");
    expect(v.ink).toBe("#0f0f17");
  });

  it("fills missing colors from background and foreground", () => {
    const v = themeVars({ name: "", mode: "light", colors: { background: "#ffffff", foreground: "#000000" } });
    expect(v.bar).toContain("color-mix");
    expect(v.accent).toContain("color-mix");
    // light theme: text on accent uses the page background
    expect(v.ink).toBe("#ffffff");
  });
});
