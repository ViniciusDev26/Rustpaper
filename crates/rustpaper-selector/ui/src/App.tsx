import { useEffect, useState } from "react";
import { Button, Toast, field } from "./components/common";
import { LibraryView } from "./components/LibraryView";
import { WorkshopView } from "./components/WorkshopView";
import { useLibrary, visibleWallpapers, type SortMode } from "./store/library";
import { useSettings, volumeText } from "./store/settings";
import { errorText, toast } from "./store/toast";
import { api, onThemeChanged } from "./api/tauri";
import { applyTheme } from "./lib/theme";

type Tab = "library" | "store";

function VolumeSlider() {
  const { volume, setVolume } = useSettings();
  const [shown, setShown] = useState(volume);
  useEffect(() => setShown(volume), [volume]);
  return (
    <label className="flex items-center gap-1.5 text-sm whitespace-nowrap text-sub" title="Volume of the wallpapers' own music and sounds (0 = muted)">
      {volumeText(shown)}
      <input
        type="range"
        className="w-[90px]"
        min={0}
        max={100}
        step={5}
        value={shown}
        // the label follows the drag; the engine restarts only on release
        onChange={(e) => setShown(Number(e.currentTarget.value))}
        onPointerUp={(e) => setVolume(Number(e.currentTarget.value))}
        onKeyUp={(e) => setVolume(Number(e.currentTarget.value))}
      />
    </label>
  );
}

function Header({ tab, setTab }: { tab: Tab; setTab: (t: Tab) => void }) {
  const lib = useLibrary();
  const settings = useSettings();
  const count = visibleWallpapers(lib).length;
  const check = "flex cursor-pointer items-center gap-1.5 text-sm whitespace-nowrap text-sub";
  return (
    // No title bar under Hyprland (tauri.conf.json `decorations: false`): this
    // header is the top of the window, and its empty parts drag it when the
    // window floats.
    <header data-tauri-drag-region className="flex flex-wrap items-center gap-3 gap-y-2 border-b border-line bg-bar px-5 py-3 select-none">
      <h1 data-tauri-drag-region className="m-0 text-base font-semibold">
        Rustpaper
      </h1>
      <nav className="flex gap-1">
        <Button variant={tab === "library" ? "tab-active" : "tab"} onClick={() => setTab("library")}>
          Library
        </Button>
        <Button
          variant={tab === "store" ? "tab-active" : "tab"}
          title="Browse and subscribe on the Steam Workshop (needs Steam running)"
          onClick={() => setTab("store")}
        >
          Workshop
        </Button>
      </nav>
      {tab === "library" && <span className="text-sm whitespace-nowrap text-muted">{count} wallpapers</span>}
      <span data-tauri-drag-region className="flex-1 self-stretch" />
      {tab === "library" && (
        <>
          <input
            type="search"
            className={`${field} w-44`}
            placeholder="Search"
            value={lib.search}
            onChange={(e) => lib.setSearch(e.currentTarget.value)}
          />
          <select
            className={field}
            title="Order of the library"
            value={lib.sort}
            onChange={(e) => void lib.setSort(e.currentTarget.value as SortMode)}
          >
            <option value="title">Title (A–Z)</option>
            <option value="subscribed">Recently subscribed</option>
          </select>
        </>
      )}
      <label className={check} title="SceneScripts run in a sandbox (no files, network or processes)">
        <input type="checkbox" checked={settings.scripts} onChange={(e) => settings.setScripts(e.currentTarget.checked)} />
        Run scripts
      </label>
      <VolumeSlider />
      <label className={check}>
        <input type="checkbox" checked={settings.autostart} onChange={(e) => settings.setAutostart(e.currentTarget.checked)} />
        Start on login
      </label>
    </header>
  );
}

export function App() {
  const [tab, setTab] = useState<Tab>("library");

  useEffect(() => {
    // follow the Omarchy theme (and its switches); defaults elsewhere
    api.getTheme().then(applyTheme, () => {});
    const unlisten = onThemeChanged(applyTheme);
    return () => {
      void unlisten.then((f) => f());
    };
  }, []);

  useEffect(() => {
    // settings first: the panel reopens the last applied wallpaper
    (async () => {
      try {
        await useSettings.getState().load();
        await useLibrary.getState().load();
        const last = useSettings.getState().lastId;
        if (last && useLibrary.getState().wallpapers.some((w) => w.id === last)) useLibrary.getState().select(last);
      } catch (e) {
        toast(errorText(e));
      }
    })();
  }, []);

  const showTab = (t: Tab) => {
    setTab(t);
    // back to the Library: pick up downloads finished in the store
    if (t === "library") useLibrary.getState().reload().catch((e) => toast(errorText(e)));
  };

  return (
    <div className="flex h-full flex-col">
      <Header tab={tab} setTab={showTab} />
      <main className="flex min-h-0 flex-1">
        {tab === "library" ? <LibraryView /> : <WorkshopView onApplied={() => showTab("library")} />}
      </main>
      <Toast />
    </div>
  );
}
