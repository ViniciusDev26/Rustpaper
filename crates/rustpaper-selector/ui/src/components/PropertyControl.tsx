// One control per Wallpaper Engine property type (slider, bool, color, combo,
// text, group header, notes). Author texts are rendered as React text only.
import { useEffect, useState } from "react";
import type { Property } from "../api/schemas";
import { api } from "../api/tauri";
import { colorHint, fromHex, isHex, label, plainText, toHex } from "../lib/properties";
import { field } from "./common";

function Row({ p, extra, children }: { p: Property; extra?: React.ReactNode; children?: React.ReactNode }) {
  return (
    <div className="py-[7px]">
      <div className="mb-1 flex justify-between gap-2 text-[0.8rem] text-sub">
        <span>{label(p)}</span>
        {extra}
      </div>
      {children}
    </div>
  );
}

function Slider({ p, value, set }: { p: Property; value: unknown; set: (v: unknown) => void }) {
  const initial = Number(value ?? p.value ?? 0);
  const [shown, setShown] = useState(initial);
  const fmt = (v: number) => (p.fraction === false ? String(Math.round(v)) : v.toFixed(p.precision ?? 2));
  return (
    <Row p={p} extra={<span className="tabular-nums text-muted">{fmt(shown)}</span>}>
      <input
        type="range"
        className="w-full"
        min={p.min ?? 0}
        max={p.max ?? 1}
        step={p.step ?? (p.fraction === false ? 1 : 0.01)}
        defaultValue={initial}
        onInput={(e) => setShown(Number(e.currentTarget.value))}
        // commit on release: every save restarts the wallpaper
        onPointerUp={(e) => set(Number(e.currentTarget.value))}
        onKeyUp={(e) => set(Number(e.currentTarget.value))}
      />
    </Row>
  );
}

/// Dominant colors of each wallpaper's preview, fetched once per session.
const paletteCache = new Map<string, Promise<string[]>>();
function wallpaperPalette(id: string): Promise<string[]> {
  let p = paletteCache.get(id);
  if (!p) {
    p = api.wallpaperColors(id).catch(() => []);
    paletteCache.set(id, p);
  }
  return p;
}

function Swatch({ hex, active, title, onClick }: { hex: string; active: boolean; title: string; onClick: () => void }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      className={`h-6 w-6 flex-none cursor-pointer rounded-full border-2 ${active ? "border-fg" : "border-edge"}`}
      style={{ background: hex }}
    />
  );
}

/// A color property (WE's "r g b"): the current color with its hex code, a
/// picker, the author's default, and colors taken from the wallpaper itself
/// (one click applies them). Scheme color gets a short explanation: users
/// saw a lone color picker and did not know what it changes.
function ColorControl({
  p,
  value,
  set,
  wallpaperId,
}: {
  p: Property;
  value: unknown;
  set: (v: unknown) => void;
  wallpaperId: string;
}) {
  const current = toHex(value ?? p.value);
  const fallback = toHex(p.value);
  const [text, setText] = useState(current);
  const [palette, setPalette] = useState<string[]>([]);
  useEffect(() => setText(current), [current]);
  useEffect(() => {
    let live = true;
    void wallpaperPalette(wallpaperId).then((c) => live && setPalette(c));
    return () => {
      live = false;
    };
  }, [wallpaperId]);
  const pick = (hex: string) => {
    setText(hex);
    set(fromHex(hex));
  };
  const hint = colorHint(p);
  return (
    <Row p={p}>
      <div className="flex items-center gap-2">
        <input
          type="color"
          title="Pick any color"
          className="h-8 w-10 flex-none cursor-pointer rounded-md border border-edge bg-card p-0.5"
          value={current}
          onChange={(e) => pick(e.currentTarget.value)}
        />
        <input
          type="text"
          spellCheck={false}
          className={`${field} w-24 font-mono uppercase`}
          value={text}
          onChange={(e) => setText(e.currentTarget.value)}
          onBlur={() => (isHex(text) ? pick(text.toLowerCase()) : setText(current))}
          onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
        />
        <Swatch hex={fallback} active={current === fallback} title={`Default (${fallback})`} onClick={() => pick(fallback)} />
        <span className="text-[0.7rem] text-muted">default</span>
      </div>
      {palette.length > 0 && (
        <div className="mt-2">
          <div className="mb-1 text-[0.7rem] text-muted">From this wallpaper</div>
          <div className="flex flex-wrap gap-1.5">
            {palette.map((hex) => (
              <Swatch key={hex} hex={hex} active={current === hex} title={hex} onClick={() => pick(hex)} />
            ))}
          </div>
        </div>
      )}
      {hint && <div className="mt-1.5 text-[0.72rem] leading-snug text-muted">{hint}</div>}
    </Row>
  );
}

/// Control for property `p` showing `value`; `set(v)` commits a new value.
/// Returns null for properties with nothing to show.
export function PropertyControl({
  p,
  value,
  set,
  wallpaperId,
}: {
  p: Property;
  value: unknown;
  set: (v: unknown) => void;
  wallpaperId: string;
}) {
  const type = String(p.type ?? "").toLowerCase();
  switch (type) {
    case "slider":
      return <Slider p={p} value={value} set={set} />;
    case "bool":
      return (
        <label className="flex cursor-pointer items-center gap-2 py-[7px] text-[0.8rem] text-sub">
          <input type="checkbox" checked={!!value} onChange={(e) => set(e.currentTarget.checked)} />
          {label(p)}
        </label>
      );
    case "color":
      return <ColorControl p={p} value={value} set={set} wallpaperId={wallpaperId} />;
    case "combo": {
      const options = p.options ?? [];
      // eslint-disable-next-line eqeqeq
      const current = options.findIndex((o) => o.value == value);
      return (
        <Row p={p}>
          <select
            className={`${field} w-full`}
            value={current < 0 ? "" : String(current)}
            onChange={(e) => set(options[Number(e.currentTarget.value)].value)}
          >
            {current < 0 && <option value="" />}
            {options.map((o, i) => (
              <option key={i} value={String(i)}>
                {plainText(o.label ?? o.value)}
              </option>
            ))}
          </select>
        </Row>
      );
    }
    case "textinput":
    case "text":
      return (
        <Row p={p}>
          <input
            type="text"
            className={`${field} w-full`}
            defaultValue={value == null ? "" : String(value)}
            onBlur={(e) => set(e.currentTarget.value)}
            onKeyDown={(e) => e.key === "Enter" && set(e.currentTarget.value)}
          />
        </Row>
      );
    case "group":
      return (
        <div className="border-b border-line pt-3.5 text-xs uppercase tracking-widest text-accent">{label(p)}</div>
      );
    case "file":
    case "directory":
      return (
        <div className="py-[7px] text-[0.78rem] text-muted">
          {label(p)} (custom files are not supported yet)
        </div>
      );
    default: {
      // untyped entries are usually separators / notes from the author
      const text = label(p);
      return text ? <div className="py-[7px] text-[0.78rem] text-muted">{text}</div> : null;
    }
  }
}
