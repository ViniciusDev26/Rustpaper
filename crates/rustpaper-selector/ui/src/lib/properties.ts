// Wallpaper Engine user properties (project.json `general.properties`):
// labels, color conversion and `condition` expressions.
//
// Security: property texts come from Workshop authors. They are rendered as
// React text (never as HTML), and conditions are evaluated by the small
// parser below, never with eval/Function: wallpaper-provided code is not
// executed.
import type { Property } from "../api/schemas";

/// WE ships localization keys for its stock properties.
const KNOWN: Record<string, string> = {
  ui_browse_properties_scheme_color: "Scheme color",
  ui_browse_properties_alignment: "Alignment",
  ui_browse_properties_background_color: "Background color",
  ui_browse_properties_sound_volume: "Sound volume",
  ui_browse_properties_audio_volume: "Audio volume",
  ui_browse_properties_play_speed: "Playback speed",
};

const ENTITIES: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " " };

/// Author text (may contain simple HTML) -> plain text: tags dropped,
/// `<br>` as a space, common entities decoded. Pure, so it also runs in tests.
export function plainText(text: unknown): string {
  if (text == null) return "";
  return String(text)
    .replace(/<br\s*\/?>/gi, " ")
    .replace(/<[^>]*>/g, "")
    .replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (m, e: string) => {
      if (e[0] === "#") {
        const code = e[1] === "x" || e[1] === "X" ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10);
        return Number.isFinite(code) ? String.fromCodePoint(code) : m;
      }
      return ENTITIES[e.toLowerCase()] ?? m;
    })
    .replace(/\s+/g, " ")
    .trim();
}

export function label(p: Pick<Property, "name" | "text">): string {
  const raw = p.text ?? p.name;
  if (KNOWN[raw]) return KNOWN[raw];
  if (/^ui_[a-z0-9_]+$/i.test(raw)) {
    const s = raw.replace(/^ui_(browse_properties_)?/i, "").replace(/_/g, " ");
    return s.charAt(0).toUpperCase() + s.slice(1);
  }
  return plainText(raw) || p.name;
}

// ---- colors: WE stores "r g b" floats in 0..1 ------------------------------

export function toHex(value: unknown): string {
  const c = String(value ?? "")
    .trim()
    .split(/[\s,]+/)
    .map(Number);
  if (c.length < 3 || c.some((x) => !Number.isFinite(x))) return "#ffffff";
  const scale = c.some((x) => x > 1) ? 1 : 255; // a few store 0..255
  return (
    "#" +
    c
      .slice(0, 3)
      .map((x) =>
        Math.round(Math.min(255, Math.max(0, x * scale)))
          .toString(16)
          .padStart(2, "0"),
      )
      .join("")
  );
}

export function fromHex(hex: string): string {
  return [1, 3, 5].map((i) => (parseInt(hex.slice(i, i + 2), 16) / 255).toFixed(5)).join(" ");
}

// ---- conditions --------------------------------------------------------------
// Grammar (what WE writes): or := and ('||' and)*; and := not ('&&' not)*;
// not := '!' not | cmp; cmp := atom (op atom)?; atom := '(' or ')' |
// name('.value')? | number | 'string' | "string" | true | false.

function tokenize(src: string): string[] {
  const re = /\s*(\|\||&&|===|!==|==|!=|<=|>=|[!<>()]|-?\d+(?:\.\d+)?|"[^"]*"|'[^']*'|[A-Za-z_][\w.]*)/y;
  const out: string[] = [];
  re.lastIndex = 0;
  while (re.lastIndex < src.length) {
    const at = re.lastIndex;
    const m = re.exec(src);
    if (!m) {
      if (/^\s*$/.test(src.slice(at))) break;
      throw new Error(`bad condition near ${src.slice(at)}`);
    }
    out.push(m[1]);
  }
  return out;
}

const CMP = ["==", "!=", "===", "!==", "<", ">", "<=", ">="];

/// Evaluates a property's `condition` against the current values; an
/// unparseable condition shows the control rather than hiding it.
export function evalCondition(src: string, values: Record<string, unknown>): boolean {
  try {
    const t = tokenize(src);
    let i = 0;
    const peek = () => t[i];
    const next = () => t[i++];
    const atom = (): unknown => {
      const tok = next();
      if (tok === undefined) throw new Error("unexpected end");
      if (tok === "(") {
        const v = or();
        if (next() !== ")") throw new Error("missing )");
        return v;
      }
      if (tok === "true") return true;
      if (tok === "false") return false;
      if (/^-?\d/.test(tok)) return Number(tok);
      if (/^["']/.test(tok)) return tok.slice(1, -1);
      return values[tok.replace(/\.value$/, "")];
    };
    const cmp = (): unknown => {
      const a = atom();
      const op = peek();
      if (!CMP.includes(op)) return a;
      next();
      const b = atom();
      // WE compares loosely ("3" == 3; combo values are often strings)
      // eslint-disable-next-line eqeqeq
      const eq = a == b;
      switch (op) {
        case "==":
        case "===":
          return eq;
        case "!=":
        case "!==":
          return !eq;
        case "<":
          return Number(a) < Number(b);
        case ">":
          return Number(a) > Number(b);
        case "<=":
          return Number(a) <= Number(b);
        default:
          return Number(a) >= Number(b);
      }
    };
    const not = (): unknown => {
      if (peek() === "!") {
        next();
        return !not();
      }
      return cmp();
    };
    const and = (): unknown => {
      let v = not();
      while (peek() === "&&") {
        next();
        const r = not();
        v = v && r;
      }
      return v;
    };
    const or = (): unknown => {
      let v = and();
      while (peek() === "||") {
        next();
        const r = and();
        v = v || r;
      }
      return v;
    };
    const v = or();
    if (i !== t.length) throw new Error("trailing tokens");
    return !!v;
  } catch {
    return true;
  }
}

export function sameValue(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}
