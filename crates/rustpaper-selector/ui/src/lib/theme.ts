// Omarchy theme -> the UI's color tokens (the --color-* custom properties
// Tailwind's theme reads, defined in index.css with the default Catppuccin
// palette). Without an Omarchy theme the defaults stay.
import type { Theme } from "../api/schemas";

const TOKENS = ["bg", "bar", "card", "line", "edge", "surface", "muted", "sub", "fg", "accent", "pink", "red", "green", "yellow", "ink"] as const;
type Token = (typeof TOKENS)[number];

/// Token values for a theme. Omarchy's palette is semantic (background,
/// foreground, accent...); tokens it has no direct match for are mixed from
/// it, so a light theme stays readable too.
export function themeVars(t: Theme): Record<Token, string> {
  const c = t.colors;
  const pick = (...names: string[]) => names.map((n) => c[n]).find(Boolean);
  const bg = c.background;
  const fg = c.foreground;
  const mix = (a: string, pct: number, b: string) => `color-mix(in srgb, ${a} ${pct}%, ${b})`;
  const accent = pick("accent", "blue") ?? mix(fg, 70, bg);
  return {
    bg,
    bar: pick("dark_background") ?? mix(bg, 85, fg),
    card: pick("lighter_background") ?? mix(fg, 10, bg),
    line: pick("lighter_background") ?? mix(fg, 10, bg),
    edge: pick("muted") ?? mix(fg, 20, bg),
    surface: mix(fg, 30, bg),
    muted: mix(fg, 45, bg),
    sub: pick("dark_foreground") ?? mix(fg, 75, bg),
    fg,
    accent,
    pink: pick("magenta", "bright_magenta") ?? accent,
    red: pick("red", "bright_red") ?? accent,
    green: pick("green", "bright_green") ?? accent,
    yellow: pick("yellow", "bright_yellow") ?? accent,
    // text on accent/colored badges: the darkest background in a dark
    // theme, the page background in a light one
    ink: t.mode === "dark" ? (pick("darker_background", "dark_background") ?? bg) : bg,
  };
}

/// Applies a theme to the document, or restores the defaults (null).
export function applyTheme(t: Theme | null) {
  const root = document.documentElement.style;
  if (!t) {
    for (const k of TOKENS) root.removeProperty(`--color-${k}`);
    root.removeProperty("color-scheme");
    return;
  }
  for (const [k, v] of Object.entries(themeVars(t))) root.setProperty(`--color-${k}`, v);
  root.setProperty("color-scheme", t.mode);
}
