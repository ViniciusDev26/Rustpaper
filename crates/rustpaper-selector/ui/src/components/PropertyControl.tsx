// One control per Wallpaper Engine property type (slider, bool, color, combo,
// text, group header, notes). Author texts are rendered as React text only.
import { useState } from "react";
import type { Property } from "../api/schemas";
import { fromHex, label, plainText, toHex } from "../lib/properties";
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

/// Control for property `p` showing `value`; `set(v)` commits a new value.
/// Returns null for properties with nothing to show.
export function PropertyControl({ p, value, set }: { p: Property; value: unknown; set: (v: unknown) => void }) {
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
      return (
        <Row p={p}>
          <input
            type="color"
            className="h-7 w-14 rounded-md border border-edge bg-card p-0.5"
            defaultValue={toHex(value)}
            onChange={(e) => set(fromHex(e.currentTarget.value))}
          />
        </Row>
      );
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
