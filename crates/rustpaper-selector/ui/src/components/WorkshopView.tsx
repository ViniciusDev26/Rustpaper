// The Workshop tab: search and filters, result cards with Subscribe /
// Unsubscribe / Apply and download progress, Load more.
import { useEffect, useState } from "react";
import type { WorkshopItem, WorkshopKind, WorkshopSort } from "../api/schemas";
import { useLibrary } from "../store/library";
import { fmtCount, fmtSize, hasMore, isDownloading, useWorkshop } from "../store/workshop";
import { Badge, Button, Card, Thumb, field } from "./common";

const SORTS: [WorkshopSort, string][] = [
  ["trend", "Trending this week"],
  ["top", "Most popular"],
  ["subscriptions", "Most subscribed"],
  ["recent", "Most recent"],
  ["updated", "Recently updated"],
  ["subscribed", "My subscriptions"],
];
const KINDS: [WorkshopKind, string][] = [
  ["all", "All types"],
  ["scene", "Scene"],
  ["video", "Video"],
  ["web", "Web"],
];

function Actions({ it, onApply }: { it: WorkshopItem; onApply: () => void }) {
  const { subscribe, unsubscribe } = useWorkshop();
  const s = it.state;
  const cls = "flex-1 px-1.5 py-1 text-xs";
  if (isDownloading(s)) {
    const pct = s.total ? Math.floor((100 * s.downloaded) / s.total) : 0;
    return (
      <Button variant="secondary" disabled className={cls}>
        {s.total ? `Downloading ${pct}%` : "Queued…"}
      </Button>
    );
  }
  if (s.subscribed) {
    return (
      <>
        <Button className={cls} onClick={onApply}>
          Apply
        </Button>
        <Button variant="secondary" className={cls} onClick={() => unsubscribe(it)}>
          Unsubscribe
        </Button>
      </>
    );
  }
  return (
    <Button className={cls} onClick={() => subscribe(it)}>
      Subscribe
    </Button>
  );
}

export function WorkshopView({ onApplied }: { onApplied: () => void }) {
  const ws = useWorkshop();
  const { wallpapers, apply } = useLibrary();
  const [search, setSearch] = useState(ws.search);
  useEffect(() => ws.open(), []); // eslint-disable-line react-hooks/exhaustive-deps

  const applyInstalled = (id: string) => {
    const w = wallpapers.find((x) => x.id === id);
    if (!w) return;
    onApplied();
    void apply(w);
  };

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="flex flex-wrap items-center gap-2.5 px-5 pt-3.5">
        <input
          type="search"
          className={`${field} w-64`}
          placeholder="Search the Workshop (Enter)"
          value={search}
          onChange={(e) => setSearch(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key !== "Enter") return;
            ws.setFilter({ search });
            void ws.load(true);
          }}
        />
        <select className={field} value={ws.sort} onChange={(e) => ws.setFilter({ sort: e.currentTarget.value as WorkshopSort })}>
          {SORTS.map(([v, l]) => (
            <option key={v} value={v}>
              {l}
            </option>
          ))}
        </select>
        <select className={field} value={ws.kind} onChange={(e) => ws.setFilter({ kind: e.currentTarget.value as WorkshopKind })}>
          {KINDS.map(([v, l]) => (
            <option key={v} value={v}>
              {l}
            </option>
          ))}
        </select>
        <label className="flex cursor-pointer items-center gap-1.5 text-sm whitespace-nowrap text-sub" title="Also show items rated Questionable or Mature">
          <input type="checkbox" checked={ws.mature} onChange={(e) => ws.setFilter({ mature: e.currentTarget.checked })} />
          Mature
        </label>
      </div>
      <div className="px-5 pt-2.5 text-sm text-sub">{ws.status}</div>
      <div className="grid grid-cols-[repeat(auto-fill,minmax(190px,1fr))] gap-3.5 p-5">
        {ws.items.map((it) => (
          <Card key={it.id} interactive={false}>
            <Thumb src={it.preview_url || null} />
            <div className="px-2.5 py-2">
              <div className="truncate text-sm" title={it.title}>
                {it.title || it.id}
              </div>
              <div className="mt-1 flex gap-1">
                <Badge>{it.kind}</Badge>
                {it.rating !== "everyone" && <Badge>{it.rating}</Badge>}
              </div>
              <div className="mt-1 flex justify-between gap-1.5 text-[0.72rem] text-muted">
                <span>{fmtCount(it.subscriptions)} subscribers</span>
                <span>{it.file_size ? fmtSize(it.file_size) : ""}</span>
              </div>
              <div className="mt-2 flex gap-1.5">
                <Actions it={it} onApply={() => applyInstalled(it.id)} />
              </div>
            </div>
          </Card>
        ))}
      </div>
      {hasMore(ws) && (
        <div className="pb-6 text-center">
          <Button variant="secondary" disabled={ws.loading} onClick={ws.more}>
            Load more
          </Button>
        </div>
      )}
    </div>
  );
}
