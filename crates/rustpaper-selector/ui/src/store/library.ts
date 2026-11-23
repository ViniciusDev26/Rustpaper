// The Library: wallpapers found in the Workshop folders, the selected one
// (whose properties the panel shows), search and ordering.
import { create } from "zustand";
import type { Wallpaper } from "../api/schemas";
import { api } from "../api/tauri";
import { useRoles } from "./roles";
import { errorText, toast } from "./toast";

export type SortMode = "title" | "subscribed";

const SORT_KEY = "rp-lib-sort";
function savedSort(): SortMode {
  try {
    return localStorage.getItem(SORT_KEY) === "subscribed" ? "subscribed" : "title";
  } catch {
    return "title";
  }
}

interface LibraryState {
  wallpapers: Wallpaper[];
  loading: boolean;
  error: string | null;
  selectedId: string | null;
  search: string;
  sort: SortMode;
  /// Steam subscription times (unix s) once fetched for the "subscribed" order
  subTimes: Record<string, number> | null;
  load: () => Promise<void>;
  /// Re-scan after the store downloaded or removed something.
  reload: () => Promise<void>;
  select: (id: string | null) => void;
  apply: (w: Wallpaper) => Promise<void>;
  setSearch: (q: string) => void;
  setSort: (s: SortMode) => Promise<void>;
  unsubscribe: (w: Wallpaper) => Promise<boolean>;
}

/// Steam keeps no subscription time on disk: the times come from the
/// Workshop helper (Steam running); without it the folder's creation time
/// (the download) stands in.
async function fetchSubTimes(): Promise<Record<string, number>> {
  try {
    return await api.workshopSubscriptions();
  } catch {
    toast("Steam unavailable: sorting by download date");
    return {};
  }
}

export const useLibrary = create<LibraryState>((set, get) => ({
  wallpapers: [],
  loading: true,
  error: null,
  selectedId: null,
  search: "",
  sort: savedSort(),
  subTimes: null,
  load: async () => {
    try {
      const wallpapers = await api.listWallpapers();
      const subTimes = get().sort === "subscribed" ? await fetchSubTimes() : null;
      set({ wallpapers, subTimes, loading: false, error: null });
    } catch (e) {
      set({ loading: false, error: errorText(e) });
    }
  },
  reload: async () => {
    const wallpapers = await api.listWallpapers();
    const { selectedId, sort } = get();
    const subTimes = sort === "subscribed" ? await fetchSubTimes() : get().subTimes;
    set({
      wallpapers,
      subTimes,
      selectedId: wallpapers.some((w) => w.id === selectedId) ? selectedId : null,
    });
  },
  select: (id) => set({ selectedId: id }),
  apply: async (w) => {
    try {
      await api.apply(w.id);
      toast(`Applied: ${w.title || w.id}`);
      set({ selectedId: w.id });
    } catch (e) {
      toast(`Error: ${errorText(e)}`);
    }
    // the badges show where each wallpaper is
    await useRoles.getState().load();
  },
  setSearch: (search) => set({ search }),
  setSort: async (sort) => {
    try {
      localStorage.setItem(SORT_KEY, sort);
    } catch {
      /* per-viewer convenience only */
    }
    set({ sort });
    if (sort === "subscribed" && !get().subTimes) set({ subTimes: await fetchSubTimes() });
  },
  unsubscribe: async (w) => {
    try {
      await api.workshopUnsubscribe(w.id);
      toast(`Unsubscribed: ${w.title || w.id}`);
      await get().reload();
      return true;
    } catch (e) {
      toast(errorText(e));
      return false;
    }
  },
}));

/// Search + order, as shown in the grid.
export function visibleWallpapers(s: Pick<LibraryState, "wallpapers" | "search" | "sort" | "subTimes">): Wallpaper[] {
  const q = s.search.trim().toLowerCase();
  const shown = s.wallpapers.filter(
    (w) => !q || w.title.toLowerCase().includes(q) || w.id.includes(q),
  );
  if (s.sort !== "subscribed") return shown; // backend sends title order
  // Steam's subscriptions first (by subscription time), then folders without
  // an active subscription (old installs) by download date
  const sub = (w: Wallpaper) => s.subTimes?.[w.id] ?? 0;
  return [...shown].sort(
    (a, b) => Number(sub(b) > 0) - Number(sub(a) > 0) || sub(b) - sub(a) || b.added - a.added,
  );
}
