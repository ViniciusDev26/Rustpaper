// The Library tab: the wallpapers found in the Workshop folders and the
// properties panel of the selected one. Like WE, one click applies a
// wallpaper and shows its properties: every type plays, since the real
// Wallpaper Engine renders it.
import { useMemo } from "react";
import { fileUrl } from "../api/tauri";
import { useLibrary, visibleWallpapers } from "../store/library";
import { Badge, Card, Thumb } from "./common";
import { PropertiesPanel } from "./PropertiesPanel";

export function LibraryView() {
  const { wallpapers, search, sort, subTimes, selectedId, loading, error, apply } = useLibrary();
  const shown = useMemo(
    () => visibleWallpapers({ wallpapers, search, sort, subTimes }),
    [wallpapers, search, sort, subTimes],
  );
  const selected = wallpapers.find((w) => w.id === selectedId) ?? null;

  return (
    <>
      <div className="flex-1 overflow-y-auto">
        {loading && <div className="p-5 text-sub">Loading wallpapers…</div>}
        {error && <div className="p-5 text-sub">Failed to load: {error}</div>}
        <div className="grid grid-cols-[repeat(auto-fill,minmax(190px,1fr))] gap-3.5 p-5">
          {shown.map((w) => (
            <Card key={w.id} selected={w.id === selectedId} onClick={() => apply(w)}>
              <Thumb src={w.preview ? fileUrl(w.preview) : null} />
              <div className="px-2.5 py-2">
                <div className="truncate text-sm" title={w.title}>
                  {w.title || w.id}
                </div>
                <div className="mt-1">
                  <Badge>{w.kind}</Badge>
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
