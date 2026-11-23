import { useEffect, useState } from "react";
import { Button, Toast, field } from "./components/common";
import { LibraryView } from "./components/LibraryView";
import { PerformanceView } from "./components/PerformanceView";
import { WorkshopView } from "./components/WorkshopView";
import { useLibrary, visibleWallpapers, type SortMode } from "./store/library";
import { useRoles } from "./store/roles";
import { useSettings, volumeText } from "./store/settings";
import { errorText, toast } from "./store/toast";
import { api, onThemeChanged, onWallpaperTheme } from "./api/tauri";
import { applyTheme } from "./lib/theme";

type Tab = "library" | "store" | "performance";

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
        <Button
          variant={tab === "performance" ? "tab-active" : "tab"}
          title="Quality and playback settings, like Wallpaper Engine's"
          onClick={() => setTab("performance")}
        >
          Performance
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
      <VolumeSlider />
      {settings.omarchy && (
        <label className={check} title="Generate an Omarchy theme from each applied wallpaper's colors (like Aether) and apply it to the whole desktop">
          <input
            type="checkbox"
            checked={settings.wallpaperTheme}
            onChange={(e) => settings.setWallpaperTheme(e.currentTarget.checked)}
          />
          Theme from wallpaper
        </label>
      )}
      <label
        className={check}
        title="The lock screen shows the live wallpaper behind the password prompt. Your desktop's lock key, idle lock and lock before suspend use Rustpaper's lock screen; off gives them back to the desktop's own."
      >
        <input type="checkbox" checked={settings.animatedLock} onChange={(e) => settings.setAnimatedLock(e.currentTarget.checked)} />
        Animated lock screen
      </label>
      <label className={check} title="Runs the wallpaper as a systemd user service (rustpaper.service) that starts with your session">
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
    const unlistenWallpaper = onWallpaperTheme((err) => toast(err ? `Theme: ${err}` : "Desktop theme updated"));
    return () => {
      void unlisten.then((f) => f());
      void unlistenWallpaper.then((f) => f());
    };
  }, []);

  useEffect(() => {
    api.desktopInfo().then((d) => d.start_tab && setTab(d.start_tab), () => {});
  }, []);

  useEffect(() => {
    // settings first: the panel reopens the last applied wallpaper
    (async () => {
      try {
        await useSettings.getState().load();
        await useLibrary.getState().load();
        void useRoles.getState().load();
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
        {tab === "library" ? (
          <LibraryView />
        ) : tab === "store" ? (
          <WorkshopView onApplied={() => showTab("library")} />
        ) : (
          <PerformanceView />
        )}
      </main>
      <Toast />
    </div>
  );
}
