// Workshop tab: browse the Steam Workshop for Wallpaper Engine, subscribe /
// unsubscribe, follow downloads. The backend (workshop_* commands) goes
// through rustpaper-workshop, i.e. the running Steam client; Steam itself
// downloads into the folder the Library scans, so a finished download only
// needs a library reload.
const RpStore = (() => {
  let ctx; // { invoke, el, $, showToast, reloadLibrary, applyInstalled }
  let items = []; // shown items, in order
  let page = 1;
  let total = 0;
  let loading = false;
  let opened = false;
  const cards = new Map(); // id -> { node, item }
  const watching = new Set(); // subscribed ids still downloading
  let pollTimer = null;

  const fmtCount = (n) =>
    n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(1)}k` : `${n}`;
  const fmtSize = (b) =>
    b >= 1 << 30 ? `${(b / (1 << 30)).toFixed(1)} GB` : `${Math.max(1, Math.round(b / (1 << 20)))} MB`;

  function query() {
    const { $ } = ctx;
    return {
      sort: $("ws-sort").value,
      kind: $("ws-kind").value,
      search: $("ws-search").value.trim(),
      mature: $("ws-mature").checked,
      page,
    };
  }

  function status(text) {
    ctx.$("store-status").textContent = text;
  }

  async function load(reset) {
    if (loading) return;
    loading = true;
    const { invoke, $ } = ctx;
    if (reset) {
      page = 1;
      items = [];
      cards.clear();
      $("store-grid").replaceChildren();
    }
    $("store-more").hidden = true;
    status(page === 1 ? "Loading the Workshop…" : "Loading more…");
    try {
      const res = await invoke("workshop_query", { query: query() });
      total = res.total;
      for (const it of res.items) {
        if (cards.has(it.id)) continue;
        items.push(it);
        const node = card(it);
        cards.set(it.id, { node, item: it });
        $("store-grid").append(node);
        if (it.state.subscribed && (it.state.downloading || !it.state.installed)) watch(it.id);
      }
      const more = page * 50 < total;
      $("store-more").hidden = !more;
      status(items.length ? `${fmtCount(total)} results` : "Nothing found.");
    } catch (e) {
      status(`${e}`);
    } finally {
      loading = false;
    }
  }

  function card(it) {
    const { el } = ctx;
    const thumb = it.preview_url
      ? el("img", { className: "thumb", loading: "lazy", referrerPolicy: "no-referrer", src: it.preview_url })
      : el("div", { className: "no-thumb", textContent: "no preview" });
    const badges = el("div", {}, el("span", { className: `badge ${it.kind}`, textContent: it.kind }));
    if (it.rating !== "everyone") badges.append(el("span", { className: "badge rating", textContent: it.rating }));
    const meta = el("div", { className: "meta" },
      el("div", { className: "title", title: it.title, textContent: it.title || it.id }),
      badges,
      el("div", { className: "stats" },
        el("span", { textContent: `${fmtCount(it.subscriptions)} subscribers` }),
        el("span", { textContent: it.file_size ? fmtSize(it.file_size) : "" })));
    if (it.kind === "web") meta.append(el("div", { className: "note", textContent: "Web wallpapers don't run yet" }));
    const row = el("div", { className: "row" });
    meta.append(row);
    const node = el("div", { className: "card store-card" }, thumb, meta);
    node.dataset.id = it.id;
    node._row = row;
    drawActions(node, it);
    return node;
  }

  /// Buttons for the item's local state.
  function drawActions(node, it) {
    const { el } = ctx;
    const s = it.state;
    const btn = (text, cls, onClick, disabled = false) => {
      const b = el("button", { className: cls, textContent: text, disabled });
      if (onClick) b.addEventListener("click", onClick);
      return b;
    };
    let buttons;
    if (s.subscribed && (s.downloading || !s.installed)) {
      const pct = s.total ? Math.floor((100 * s.downloaded) / s.total) : 0;
      buttons = [btn(s.total ? `Downloading ${pct}%` : "Queued…", "secondary", null, true)];
    } else if (s.subscribed) {
      buttons = [];
      if (it.kind !== "web") buttons.push(btn("Apply", "", () => ctx.applyInstalled(it.id)));
      buttons.push(btn("Unsubscribe", "secondary", () => unsubscribe(it)));
    } else {
      buttons = [btn("Subscribe", "", () => subscribe(it))];
    }
    node._row.replaceChildren(...buttons);
  }

  function update(id, state) {
    const c = cards.get(id);
    if (!c) return;
    c.item.state = state;
    drawActions(c.node, c.item);
  }

  async function subscribe(it) {
    try {
      update(it.id, { ...it.state, subscribed: true, downloading: true, downloaded: 0, total: 0 });
      await ctx.invoke("workshop_subscribe", { id: it.id });
      watch(it.id);
    } catch (e) {
      update(it.id, { ...it.state, subscribed: false, downloading: false });
      ctx.showToast(`${e}`);
    }
  }

  async function unsubscribe(it) {
    try {
      await ctx.invoke("workshop_unsubscribe", { id: it.id });
      watching.delete(it.id);
      update(it.id, { subscribed: false, installed: false, downloading: false, needs_update: false, downloaded: 0, total: 0 });
      ctx.showToast(`Unsubscribed: ${it.title}`);
      await ctx.reloadLibrary();
    } catch (e) {
      ctx.showToast(`${e}`);
    }
  }

  // Downloads: poll their state once a second; a finished one reloads the
  // Library so the wallpaper is there to apply.
  function watch(id) {
    watching.add(id);
    if (!pollTimer) pollTimer = setInterval(poll, 1000);
  }

  async function poll() {
    if (!watching.size) {
      clearInterval(pollTimer);
      pollTimer = null;
      return;
    }
    let states;
    try {
      states = await ctx.invoke("workshop_status", { ids: [...watching] });
    } catch {
      return; // helper restarting: try again next tick
    }
    let finished = [];
    for (const [id, s] of Object.entries(states)) {
      update(id, s);
      if (s.installed && !s.downloading) finished.push(id);
      if (!s.subscribed) watching.delete(id);
    }
    if (finished.length) {
      for (const id of finished) watching.delete(id);
      await ctx.reloadLibrary().catch(() => {});
      const titles = finished.map((id) => cards.get(id)?.item.title || id);
      ctx.showToast(`Installed: ${titles.join(", ")}`);
    }
  }

  return {
    init(c) {
      ctx = c;
      const { $ } = ctx;
      $("ws-search").addEventListener("keydown", (e) => e.key === "Enter" && load(true));
      for (const id of ["ws-sort", "ws-kind", "ws-mature"]) $(id).addEventListener("change", () => load(true));
      $("store-more").addEventListener("click", () => {
        page += 1;
        load(false);
      });
    },
    /// First visit loads the default page; later visits keep the results.
    open() {
      if (!opened) {
        opened = true;
        load(true);
      }
    },
  };
})();
