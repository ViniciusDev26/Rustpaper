// Customization panel (like WE's right-hand "Properties" column) for the
// selected wallpaper: Apply / Reset / Unsubscribe, then one control per
// property whose `condition` holds. Changes are saved (debounced) and the
// backend restarts the running wallpaper with them.
import { useEffect, useMemo, useRef, useState } from "react";
import type { PropertiesView, Wallpaper } from "../api/schemas";
import { api, fileUrl } from "../api/tauri";
import { evalCondition, sameValue } from "../lib/properties";
import { parsePlacementKey, placementKey, placementsOf, removable, targets } from "../lib/roles";
import { useLibrary } from "../store/library";
import { useRoles } from "../store/roles";
import { useSettings } from "../store/settings";
import { errorText, toast } from "../store/toast";
import { Badge, Button, ConfirmButton, field } from "./common";
import { PropertyControl } from "./PropertyControl";

/// "Use for": puts the wallpaper on the desktop of one monitor, the lock
/// screen or the screensaver, and lists (removably) where it already is.
function UseFor({ wallpaper: w }: { wallpaper: Wallpaper }) {
  const { roles, assign, clear } = useRoles();
  if (!roles) return null;
  const here = placementsOf(w.id, roles);
  const hereKeys = new Set(here.map(placementKey));
  const options = targets(roles).filter((t) => !hereKeys.has(placementKey(t)));
  return (
    <div className="px-4 pb-3">
      <select
        className={`${field} w-full`}
        value=""
        title="Show this wallpaper somewhere else too: one monitor's desktop, the lock screen, or the screensaver"
        onChange={(e) => {
          const t = parsePlacementKey(e.currentTarget.value);
          if (t) void assign(w.id, w.title, t.role, t.output);
        }}
      >
        <option value="" disabled>
          Use for…
        </option>
        {options.map((t) => (
          <option key={placementKey(t)} value={placementKey(t)}>
            {t.label}
          </option>
        ))}
      </select>
      {here.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {here.map((p) => (
            <span key={placementKey(p)} className="inline-flex items-center gap-1 rounded bg-edge px-2 py-0.5 text-[0.72rem]">
              {p.label}
              {removable(p) && (
                <button
                  className="cursor-pointer text-muted hover:text-fg"
                  title={`Stop using it for ${p.label}`}
                  onClick={() => void clear(p.role, p.output)}
                >
                  ×
                </button>
              )}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

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
  }, [w.id]);

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

  const omarchy = useSettings((s) => s.omarchy);
  const [theming, setTheming] = useState(false);
  const themeButton = omarchy && w.preview && (
    <Button
      variant="secondary"
      disabled={theming}
      title="Generate an Omarchy theme from this wallpaper's colors and apply it to the desktop"
      onClick={async () => {
        setTheming(true);
        toast("Theming the desktop from the wallpaper…");
        try {
          await api.themeFromWallpaper(w.id);
          toast("Desktop theme updated");
        } catch (e) {
          toast(`Theme: ${errorText(e)}`);
        } finally {
          setTheming(false);
        }
      }}
    >
      {theming ? "Theming…" : "Use as theme"}
    </Button>
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
      <div className="flex flex-wrap gap-2 px-4 pb-3">
        <Button onClick={() => apply(w)}>Apply</Button>
        <Button variant="secondary" onClick={reset} disabled={!view}>
          Reset
        </Button>
        {themeButton}
        {unsubscribeButton}
      </div>
      <UseFor wallpaper={w} />
      <div className="px-4 pt-1 pb-5" key={generation}>
        {error ? (
          <div className="text-sm text-muted">{error}</div>
        ) : !view ? null : shown.length === 0 ? (
          <div className="text-sm text-muted">No customizable properties.</div>
        ) : (
          shown.map((p) => (
            <PropertyControl
              key={p.name}
              p={p}
              value={values[p.name]}
              set={(v) => setValue(p.name, v)}
              wallpaperId={w.id}
            />
          ))
        )}
      </div>
    </aside>
  );
}
