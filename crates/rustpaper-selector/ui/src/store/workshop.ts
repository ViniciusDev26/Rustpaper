// The Workshop tab: query, results, subscriptions and download progress.
// The backend talks to the running Steam client (rustpaper-workshop); Steam
// downloads into the folder the Library scans, so a finished download only
// needs a Library reload.
import { create } from "zustand";
import type { ItemState, WorkshopItem, WorkshopKind, WorkshopSort } from "../api/schemas";
import { api } from "../api/tauri";
import { useLibrary } from "./library";
import { errorText, toast } from "./toast";

const PAGE_SIZE = 50;

interface WorkshopState {
  sort: WorkshopSort;
  kind: WorkshopKind;
  search: string;
  mature: boolean;
  page: number;
  total: number;
  items: WorkshopItem[];
  loading: boolean;
  status: string;
  opened: boolean;
  /// subscribed ids still downloading (polled every second)
  watching: string[];
  setFilter: (f: Partial<Pick<WorkshopState, "sort" | "kind" | "mature" | "search">>) => void;
  open: () => void;
  load: (reset: boolean) => Promise<void>;
  more: () => void;
  subscribe: (it: WorkshopItem) => Promise<void>;
  unsubscribe: (it: WorkshopItem) => Promise<void>;
}

const IDLE: ItemState = {
  subscribed: false,
  installed: false,
  downloading: false,
  needs_update: false,
  downloaded: 0,
  total: 0,
};

export const fmtCount = (n: number) =>
  n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(1)}k` : `${n}`;
export const fmtSize = (b: number) =>
  b >= 2 ** 30 ? `${(b / 2 ** 30).toFixed(1)} GB` : `${Math.max(1, Math.round(b / 2 ** 20))} MB`;
export const isDownloading = (s: ItemState) => s.subscribed && (s.downloading || !s.installed);

let pollTimer: ReturnType<typeof setInterval> | undefined;

export const useWorkshop = create<WorkshopState>((set, get) => {
  const setState = (id: string, state: ItemState) =>
    set({ items: get().items.map((it) => (it.id === id ? { ...it, state } : it)) });

  const watch = (id: string) => {
    if (!get().watching.includes(id)) set({ watching: [...get().watching, id] });
    if (!pollTimer) pollTimer = setInterval(poll, 1000);
  };

  async function poll() {
    const watching = get().watching;
    if (!watching.length) {
      clearInterval(pollTimer);
      pollTimer = undefined;
      return;
    }
    let states: Record<string, ItemState>;
    try {
      states = await api.workshopStatus(watching);
    } catch {
      return; // helper restarting: next tick
    }
    const finished: string[] = [];
    const dropped: string[] = [];
    for (const [id, s] of Object.entries(states)) {
      setState(id, s);
      if (s.installed && !s.downloading) finished.push(id);
      else if (!s.subscribed) dropped.push(id);
    }
    if (finished.length || dropped.length) {
      set({ watching: get().watching.filter((id) => !finished.includes(id) && !dropped.includes(id)) });
    }
    if (finished.length) {
      await useLibrary.getState().reload().catch(() => {});
      const titles = finished.map((id) => get().items.find((it) => it.id === id)?.title || id);
      toast(`Installed: ${titles.join(", ")}`);
    }
  }

  return {
    sort: "trend",
    kind: "all",
    search: "",
    mature: false,
    page: 1,
    total: 0,
    items: [],
    loading: false,
    status: "",
    opened: false,
    watching: [],
    setFilter: (f) => {
      set(f);
      if (f.search === undefined) void get().load(true); // search applies on Enter
    },
    open: () => {
      if (get().opened) return;
      set({ opened: true });
      void get().load(true);
    },
    load: async (reset) => {
      if (get().loading) return;
      const page = reset ? 1 : get().page;
      set({ loading: true, status: page === 1 ? "Loading the Workshop…" : "Loading more…", page });
      if (reset) set({ items: [] });
      try {
        const { sort, kind, search, mature } = get();
        const res = await api.workshopQuery({ sort, kind, search: search.trim(), mature, page });
        const known = new Set(get().items.map((it) => it.id));
        const items = [...get().items, ...res.items.filter((it) => !known.has(it.id))];
        set({ items, total: res.total, status: items.length ? `${fmtCount(res.total)} results` : "Nothing found." });
        for (const it of res.items) if (isDownloading(it.state)) watch(it.id);
      } catch (e) {
        set({ status: errorText(e) });
      } finally {
        set({ loading: false });
      }
    },
    more: () => {
      if (get().page * PAGE_SIZE >= get().total) return;
      set({ page: get().page + 1 });
      void get().load(false);
    },
    subscribe: async (it) => {
      setState(it.id, { ...it.state, subscribed: true, downloading: true, downloaded: 0, total: 0 });
      try {
        await api.workshopSubscribe(it.id);
        watch(it.id);
      } catch (e) {
        setState(it.id, { ...it.state, subscribed: false, downloading: false });
        toast(errorText(e));
      }
    },
    unsubscribe: async (it) => {
      try {
        await api.workshopUnsubscribe(it.id);
        set({ watching: get().watching.filter((id) => id !== it.id) });
        setState(it.id, IDLE);
        toast(`Unsubscribed: ${it.title}`);
        await useLibrary.getState().reload();
      } catch (e) {
        toast(errorText(e));
      }
    },
  };
});

export const hasMore = (s: Pick<WorkshopState, "page" | "total">) => s.page * PAGE_SIZE < s.total;
