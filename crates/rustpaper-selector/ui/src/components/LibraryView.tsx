// The Library tab: the wallpapers found in the Workshop folders and the
// properties panel of the selected one. One click selects a wallpaper (its
// preview, properties and "Apply to"); nothing changes on screen until it
// is applied, so a lock screen or screensaver wallpaper never passes
// through the desktop. A double click applies it to the chosen target.
import { useMemo } from "react";
import { fileUrl } from "../api/tauri";
import { placementsOf } from "../lib/roles";
import { useLibrary, visibleWallpapers } from "../store/library";
import { useRoles } from "../store/roles";
import { Badge, Card, Thumb } from "./common";
import { PropertiesPanel } from "./PropertiesPanel";

export function LibraryView() {
  const { wallpapers, search, sort, subTimes, selectedId, loading, error, select, applyToTarget } = useLibrary();
  const shown = useMemo(
    () => visibleWallpapers({ wallpapers, search, sort, subTimes }),
    [wallpapers, search, sort, subTimes],
  );
  const selected = wallpapers.find((w) => w.id === selectedId) ?? null;
  const roles = useRoles((s) => s.roles);

  return (
    <>
      <div className="flex-1 overflow-y-auto">
        {loading && <div className="p-5 text-sub">Loading wallpapers…</div>}
        {error && <div className="p-5 text-sub">Failed to load: {error}</div>}
        <div className="grid grid-cols-[repeat(auto-fill,minmax(190px,1fr))] gap-3.5 p-5">
          {shown.map((w) => (
            <Card
              key={w.id}
              selected={w.id === selectedId}
              onClick={() => select(w.id)}
              onDoubleClick={() => void applyToTarget(w)}
              title="Click to select, double-click to apply"
            >
              <Thumb src={w.preview ? fileUrl(w.preview) : null} />
              <div className="px-2.5 py-2">
                <div className="truncate text-sm" title={w.title}>
                  {w.title || w.id}
                </div>
                <div className="mt-1 flex flex-wrap items-center gap-1">
                  <Badge>{w.kind}</Badge>
                  {roles &&
                    placementsOf(w.id, roles).map((p) => (
                      <span key={p.label} className="rounded bg-edge px-1.5 py-0.5 text-[0.62rem] text-sub">
                        {p.label}
                      </span>
                    ))}
                </div>
              </div>
            </Card>
          ))}
        </div>
      </div>
      {selected && <PropertiesPanel key={selected.id} wallpaper={selected} />}
    </>
  );
}
