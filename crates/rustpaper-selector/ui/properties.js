// Controls for Wallpaper Engine user properties (project.json
// `general.properties`) and their `condition` expressions.
//
// Security: property texts come from Workshop authors. They are never
// inserted as HTML (labels go through DOMParser -> textContent, which does not
// run scripts), and conditions are evaluated by the small parser below, never
// with eval/Function — wallpaper-provided code is not executed.

"use strict";

const RpProps = (() => {
  // ---- labels ----------------------------------------------------------
  // WE ships localization keys for its stock properties.
  const KNOWN = {
    ui_browse_properties_scheme_color: "Scheme color",
    ui_browse_properties_alignment: "Alignment",
    ui_browse_properties_background_color: "Background color",
    ui_browse_properties_sound_volume: "Sound volume",
    ui_browse_properties_audio_volume: "Audio volume",
    ui_browse_properties_play_speed: "Playback speed",
  };

  function plainText(text) {
    if (text == null) return "";
    const doc = new DOMParser().parseFromString(`<body>${String(text).replace(/<br\s*\/?>/gi, " ")}</body>`, "text/html");
    return doc.body.textContent.replace(/\s+/g, " ").trim();
  }

  function label(p) {
    const raw = p.text ?? p.name;
    if (KNOWN[raw]) return KNOWN[raw];
    if (/^ui_[a-z0-9_]+$/i.test(raw)) {
      const s = raw.replace(/^ui_(browse_properties_)?/i, "").replace(/_/g, " ");
      return s.charAt(0).toUpperCase() + s.slice(1);
    }
    return plainText(raw) || p.name;
  }

  // ---- colors: WE stores "r g b" floats in 0..1 --------------------------
  function toHex(value) {
    const c = String(value ?? "").trim().split(/[\s,]+/).map(Number);
    if (c.length < 3 || c.some((x) => !Number.isFinite(x))) return "#ffffff";
    const scale = c.some((x) => x > 1) ? 1 : 255; // a few store 0..255
    return "#" + c.slice(0, 3).map((x) => Math.round(Math.min(255, Math.max(0, x * scale))).toString(16).padStart(2, "0")).join("");
  }

  function fromHex(hex) {
    return [1, 3, 5].map((i) => (parseInt(hex.slice(i, i + 2), 16) / 255).toFixed(5)).join(" ");
  }

  // ---- conditions ------------------------------------------------------
  // Grammar (what WE writes): or := and ('||' and)*; and := not ('&&' not)*;
  // not := '!' not | cmp; cmp := atom (op atom)?; atom := '(' or ')' |
  // name('.value')? | number | 'string' | "string" | true | false.
  function tokenize(src) {
    const re = /\s*(\|\||&&|==|!=|<=|>=|===|!==|[!<>()]|-?\d+(?:\.\d+)?|"[^"]*"|'[^']*'|[A-Za-z_][\w.]*)/y;
    const out = [];
    let m;
    re.lastIndex = 0;
    while (re.lastIndex < src.length) {
      const at = re.lastIndex;
      if (!(m = re.exec(src))) {
        if (/^\s*$/.test(src.slice(at))) break;
        throw new Error(`bad condition near ${src.slice(at)}`);
      }
      out.push(m[1]);
    }
    return out;
  }

  function evalCondition(src, values) {
    try {
      const t = tokenize(src);
      let i = 0;
      const peek = () => t[i];
      const next = () => t[i++];
      const atom = () => {
        const tok = next();
        if (tok === undefined) throw new Error("unexpected end");
        if (tok === "(") { const v = or(); if (next() !== ")") throw new Error("missing )"); return v; }
        if (tok === "true") return true;
        if (tok === "false") return false;
        if (/^-?\d/.test(tok)) return Number(tok);
        if (/^["']/.test(tok)) return tok.slice(1, -1);
        const name = tok.replace(/\.value$/, "");
        return values[name];
      };
      const cmp = () => {
        const a = atom();
        const op = peek();
        if (!["==", "!=", "===", "!==", "<", ">", "<=", ">="].includes(op)) return a;
        next();
        const b = atom();
        // WE compares loosely ("3" == 3, combo values are often strings)
        const eq = a == b; // eslint-disable-line eqeqeq
        switch (op) {
          case "==": case "===": return eq;
          case "!=": case "!==": return !eq;
          case "<": return Number(a) < Number(b);
          case ">": return Number(a) > Number(b);
          case "<=": return Number(a) <= Number(b);
          default: return Number(a) >= Number(b);
        }
      };
      const not = () => (peek() === "!" ? (next(), !not()) : cmp());
      const and = () => { let v = not(); while (peek() === "&&") { next(); const r = not(); v = v && r; } return v; };
      const or = () => { let v = and(); while (peek() === "||") { next(); const r = and(); v = v || r; } return v; };
      const v = or();
      if (i !== t.length) throw new Error("trailing tokens");
      return !!v;
    } catch {
      return true; // unparseable: show the control rather than hide it
    }
  }

  // ---- controls --------------------------------------------------------
  function row(p, ...children) {
    const head = document.createElement("div");
    head.className = "label";
    const name = document.createElement("span");
    name.textContent = label(p);
    head.append(name);
    const n = document.createElement("div");
    n.className = "prop";
    n.append(head, ...children);
    return { n, head };
  }

  /// Control for property `p` showing `value`; `set(v)` commits a new value.
  /// Returns null for properties with nothing to show.
  function control(p, value, set) {
    const type = String(p.type || "").toLowerCase();
    switch (type) {
      case "slider": {
        const input = Object.assign(document.createElement("input"), {
          type: "range", min: p.min ?? 0, max: p.max ?? 1,
          step: p.step ?? (p.fraction === false ? 1 : 0.01), value: value ?? p.value ?? 0,
        });
        const { n, head } = row(p, input);
        const shown = document.createElement("span");
        shown.className = "val";
        const fmt = (v) => (p.fraction === false ? String(Math.round(v)) : Number(v).toFixed(p.precision ?? 2));
        shown.textContent = fmt(input.value);
        head.append(shown);
        input.addEventListener("input", () => (shown.textContent = fmt(input.value)));
        input.addEventListener("change", () => set(Number(input.value)));
        return n;
      }
      case "bool": {
        const input = Object.assign(document.createElement("input"), { type: "checkbox", checked: !!value });
        const { n, head } = row(p);
        head.prepend(input);
        head.style.justifyContent = "flex-start";
        head.style.cursor = "pointer";
        head.addEventListener("click", (e) => { if (e.target !== input) input.click(); });
        input.addEventListener("change", () => set(input.checked));
        return n;
      }
      case "color": {
        const input = Object.assign(document.createElement("input"), { type: "color", value: toHex(value) });
        input.addEventListener("change", () => set(fromHex(input.value)));
        return row(p, input).n;
      }
      case "combo": {
        const select = document.createElement("select");
        (p.options || []).forEach((o, i) => {
          const opt = document.createElement("option");
          opt.value = String(i);
          opt.textContent = plainText(o.label ?? o.value);
          if (o.value == value) opt.selected = true; // eslint-disable-line eqeqeq
          select.append(opt);
        });
        select.addEventListener("change", () => set(p.options[Number(select.value)].value));
        return row(p, select).n;
      }
      case "textinput":
      case "text": {
        const input = Object.assign(document.createElement("input"), { type: "text", value: value ?? "" });
        input.addEventListener("change", () => set(input.value));
        return row(p, input).n;
      }
      case "group": {
        const n = document.createElement("div");
        n.className = "prop group";
        n.textContent = label(p);
        return n;
      }
      case "file":
      case "directory": {
        const { n } = row(p);
        n.classList.add("note");
        n.append(" (custom files are not supported yet)");
        return n;
      }
      default: {
        // untyped entries are usually separators / notes from the author
        const text = label(p);
        if (!text) return null;
        const n = document.createElement("div");
        n.className = "prop note";
        n.textContent = text;
        return n;
      }
    }
  }

  return { control, evalCondition, label, toHex, fromHex };
})();
