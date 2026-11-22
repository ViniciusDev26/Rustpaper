// Customization panel (like WE's right-hand "Properties" column) for the
// selected wallpaper: Apply / Reset / Unsubscribe, then one control per
// property whose `condition` holds. Changes are saved (debounced) and the
// backend restarts the running wallpaper with them.
import { useEffect, useMemo, useRef, useState } from "react";
import type { PropertiesView, Wallpaper } from "../api/schemas";
import { api, fileUrl } from "../api/tauri";
import { evalCondition, sameValue } from "../lib/properties";
import { useLibrary } from "../store/library";
import { errorText, toast } from "../store/toast";
import { Badge, Button, ConfirmButton } from "./common";
import { PropertyControl } from "./PropertyControl";

/// Saving restarts the engine; one restart per burst of edits.
const SAVE_DELAY_MS = 700;

export function PropertiesPanel({ wallpaper: w }: { wallpaper: Wallpaper }) {
  const apply = useLibrary((s) => s.apply);
  const unsubscribe = useLibrary((s) => s.unsubscribe);
  const [view, setView] = useState<PropertiesView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [values, setValues] = useState<Record<string, unknown>>({});
  // remount controls after Reset (sliders/inputs are uncontrolled)
  const [generation, setGeneration] = useState(0);
  const saveTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    setView(null);
    setError(null);
    if (!w.supported) return;
    let current = true;
    api
      .getProperties(w.id)
      .then((v) => {
        if (!current) return; // another wallpaper was selected meanwhile
        setView(v);
        setValues(Object.fromEntries(v.properties.map((p) => [p.name, p.name in v.saved ? v.saved[p.name] : p.value])));
      })
      .catch((e) => current && setError(errorText(e)));
    return () => {
      current = false;
      clearTimeout(saveTimer.current);
    };
  }, [w.id, w.supported]);

  /// Only the values that differ from the wallpaper's defaults are saved.
  const changed = (vals: Record<string, unknown>) =>
    Object.fromEntries(
      Object.entries(vals).filter(([k, v]) => {
        const p = view?.properties.find((q) => q.name === k);
        return p && !sameValue(v, p.value);
      }),
    );

  const setValue = (name: string, v: unknown) => {
    const next = { ...values, [name]: v };
    setValues(next);
    clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => {
      api.saveProperties(w.id, changed(next)).catch((e) => toast(`Error: ${errorText(e)}`));
    }, SAVE_DELAY_MS);
  };

  const reset = async () => {
    if (!view) return;
    clearTimeout(saveTimer.current);
    setValues(Object.fromEntries(view.properties.map((p) => [p.name, p.value])));
    setGeneration((g) => g + 1);
    await api.saveProperties(w.id, {}).catch((e) => toast(`Error: ${errorText(e)}`));
  };

  const shown = useMemo(
    () => (view?.properties ?? []).filter((p) => !p.condition || evalCondition(p.condition, values)),
    [view, values],
  );

  const unsubscribeButton = w.workshop && (
    <ConfirmButton label="Unsubscribe" busyLabel="Unsubscribing…" onConfirm={() => unsubscribe(w)} />
  );

  return (
    <aside className="w-[340px] flex-none overflow-y-auto border-l border-line bg-bar">
      {w.preview && <img src={fileUrl(w.preview)} className="block aspect-[16/10] w-full bg-edge object-cover" />}
      <div className="px-4 pt-3 pb-1">
        <h2 className="mb-2 text-[0.95rem] font-semibold [overflow-wrap:anywhere]">{w.title || w.id}</h2>
        <Badge>{w.kind}</Badge>
      </div>
      {!w.supported ? (
        <>
          {unsubscribeButton && <div className="flex gap-2 px-4 pb-3">{unsubscribeButton}</div>}
          <div className="px-4 pb-4 text-sm text-muted">
            {w.kind === "web"
              ? "Web wallpapers run the author's JavaScript and are not supported."
              : "This wallpaper type is not supported."}
          </div>
        </>
      ) : (
        <>
          <div className="flex gap-2 px-4 pb-3">
            <Button onClick={() => apply(w)}>Apply</Button>
            <Button variant="secondary" onClick={reset} disabled={!view}>
              Reset
            </Button>
            {unsubscribeButton}
          </div>
          <div className="px-4 pt-1 pb-5" key={generation}>
            {error ? (
              <div className="text-sm text-muted">{error}</div>
            ) : !view ? null : shown.length === 0 ? (
              <div className="text-sm text-muted">No customizable properties.</div>
            ) : (
              shown.map((p) => (
                <PropertyControl key={p.name} p={p} value={values[p.name]} set={(v) => setValue(p.name, v)} />
              ))
            )}
          </div>
        </>
      )}
    </aside>
  );
}
