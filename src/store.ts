import { create } from "zustand";
import { api, errorMessage } from "./lib/api";
import type { AppStatus, CaptureStatus, Counts, Filter, ItemDetail, ItemSummary, Settings, Sort, UpdateInfo } from "./lib/types";

const PAGE = 100;

export interface Toast {
  id: number;
  kind: "info" | "error";
  text: string;
}

export type View = "list" | "preview" | "settings";

interface AppStore {
  items: ItemSummary[];
  total: number;
  counts: Counts | null;
  search: string;
  filter: Filter;
  sort: Sort;
  selectedId: number | null;
  detail: ItemDetail | null;
  status: AppStatus | null;
  settings: Settings | null;
  loading: boolean;
  loadError: string | null;
  toasts: Toast[];
  view: View;
  /** Bumped whenever the panel is shown, so the UI can reset focus/scroll. */
  shownSeq: number;
  update: UpdateInfo | null;
  updateProgress: string | null;

  init(): Promise<() => void>;
  refresh(): Promise<void>;
  loadMore(): Promise<void>;
  setSearch(s: string): void;
  setFilter(f: Filter): void;
  setSort(s: Sort): void;
  select(id: number | null): void;
  moveSelection(delta: number): void;
  activate(id: number, paste: boolean): Promise<void>;
  togglePin(id: number): Promise<void>;
  toggleFavorite(id: number): Promise<void>;
  setTags(id: number, tags: string[]): Promise<void>;
  remove(id: number): Promise<void>;
  clear(includePinned: boolean): Promise<void>;
  setPaused(paused: boolean): Promise<void>;
  startService(): Promise<void>;
  saveSettings(s: Settings): Promise<boolean>;
  refreshStatus(): Promise<void>;
  toast(kind: Toast["kind"], text: string): void;
  dismissToast(id: number): void;
  openView(view: View, id?: number): void;
  checkUpdate(manual: boolean): Promise<void>;
  installUpdate(): Promise<void>;
}

let toastSeq = 0;
let refreshSeq = 0;

export const useApp = create<AppStore>((set, get) => {
  const fail = (e: unknown) => get().toast("error", errorMessage(e));
  const setCapture = (capture: CaptureStatus | null) =>
    set((st) => (st.status ? { status: { ...st.status, capture } } : {}));

  const loadDetail = async (id: number | null) => {
    if (id == null) return set({ detail: null });
    try {
      const d = await api.getItem(id);
      if (get().selectedId === id) set({ detail: d });
    } catch (e) {
      if (get().selectedId === id) set({ detail: null });
      fail(e);
    }
  };

  return {
    items: [],
    total: 0,
    counts: null,
    search: "",
    filter: "all",
    sort: "newest",
    selectedId: null,
    detail: null,
    status: null,
    settings: null,
    loading: true,
    loadError: null,
    toasts: [],
    view: "list",
    shownSeq: 0,
    update: null,
    updateProgress: null,

    async init() {
      const unlisteners = await Promise.all([
        api.on("history-changed", () => void get().refresh()),
        api.on<CaptureStatus | null>("status-changed", (c) => setCapture(c ?? null)),
        api.on<{ reason: string }>("capture-skipped", (p) => get().toast("info", `Not saved — ${p.reason}`)),
        api.on<string>("backend-error", (msg) => get().toast("error", msg)),
        api.on("panel-shown", () => {
          set((st) => ({ view: "list", shownSeq: st.shownSeq + 1 }));
          void get().refresh();
          void get().checkUpdate(false);
        }),
        api.on<string>("update-progress", (msg) => set({ updateProgress: msg })),
      ]);
      try {
        const [settings] = await Promise.all([api.getSettings(), get().refreshStatus()]);
        set({ settings });
      } catch (e) {
        fail(e);
      }
      await get().refresh();
      void api.panelReady();
      void get().checkUpdate(false);
      return () => unlisteners.forEach((u) => u());
    },

    async refresh() {
      const seq = ++refreshSeq;
      const { search, filter, sort, items } = get();
      try {
        const limit = Math.max(PAGE, Math.min(items.length, 500));
        const [page, counts] = await Promise.all([
          api.listItems({ search: search || undefined, filter, sort, offset: 0, limit }),
          api.getCounts(),
        ]);
        if (seq !== refreshSeq) return; // superseded
        const sel = get().selectedId;
        const selectedId = page.items.some((i) => i.id === sel) ? sel : (page.items[0]?.id ?? null);
        set({ items: page.items, total: page.total, counts, loading: false, loadError: null, selectedId });
        if (get().view === "preview" && selectedId !== null) void loadDetail(selectedId);
      } catch (e) {
        if (seq === refreshSeq) set({ loading: false, loadError: errorMessage(e) });
      }
    },

    async loadMore() {
      const { items, total, search, filter, sort, loading } = get();
      if (loading || items.length >= total) return;
      try {
        const page = await api.listItems({ search: search || undefined, filter, sort, offset: items.length, limit: PAGE });
        const seen = new Set(get().items.map((i) => i.id));
        set({ items: [...get().items, ...page.items.filter((i) => !seen.has(i.id))], total: page.total });
      } catch (e) {
        fail(e);
      }
    },

    setSearch(search) {
      set({ search });
      void get().refresh();
    },
    setFilter(filter) {
      set({ filter, selectedId: null });
      void get().refresh();
    },
    setSort(sort) {
      set({ sort });
      void get().refresh();
    },

    select(id) {
      if (id === get().selectedId) return;
      set({ selectedId: id });
      if (get().view === "preview") {
        set({ detail: null });
        void loadDetail(id);
      }
    },

    moveSelection(delta) {
      const { items, selectedId } = get();
      if (!items.length) return;
      const idx = items.findIndex((i) => i.id === selectedId);
      const next = Math.max(0, Math.min(items.length - 1, (idx < 0 ? 0 : idx) + delta));
      get().select(items[next].id);
      if (next >= items.length - 10) void get().loadMore();
    },

    async activate(id, paste) {
      try {
        const r = await api.copyItem(id, paste);
        if (!r.hidden) get().toast("info", "Copied — press Ctrl+V to paste");
      } catch (e) {
        fail(e);
      }
    },

    async togglePin(id) {
      const it = get().items.find((i) => i.id === id) ?? get().detail;
      if (!it) return;
      try {
        await api.setPinned(id, !it.isPinned);
      } catch (e) {
        fail(e);
      }
    },

    async toggleFavorite(id) {
      const it = get().items.find((i) => i.id === id) ?? get().detail;
      if (!it) return;
      try {
        await api.setFavorite(id, !it.isFavorite);
      } catch (e) {
        fail(e);
      }
    },

    async setTags(id, tags) {
      try {
        const saved = await api.setTags(id, tags);
        set((st) => (st.detail?.id === id ? { detail: { ...st.detail, tags: saved } } : {}));
      } catch (e) {
        fail(e);
      }
    },

    async remove(id) {
      const { items } = get();
      const idx = items.findIndex((i) => i.id === id);
      const neighbour = items[idx + 1] ?? items[idx - 1];
      if (get().selectedId === id) set({ selectedId: neighbour?.id ?? null, detail: null });
      if (get().view === "preview") set({ view: "list" });
      try {
        await api.deleteItem(id);
      } catch (e) {
        fail(e);
      }
    },

    async clear(includePinned) {
      try {
        await api.clearHistory(includePinned);
      } catch (e) {
        fail(e);
      }
    },

    async setPaused(paused) {
      try {
        setCapture(await api.setPaused(paused));
      } catch (e) {
        fail(e);
      }
    },

    async startService() {
      try {
        if (!(await api.startService())) get().toast("error", "The capture service could not be started");
        await get().refreshStatus();
      } catch (e) {
        fail(e);
      }
    },

    async saveSettings(s) {
      try {
        set({ settings: await api.updateSettings(s) });
        void get().refreshStatus();
        return true;
      } catch (e) {
        fail(e);
        return false;
      }
    },

    async refreshStatus() {
      try {
        set({ status: await api.getStatus() });
      } catch (e) {
        fail(e);
      }
    },

    toast(kind, text) {
      const id = ++toastSeq;
      set((st) => ({ toasts: [...st.toasts.slice(-2), { id, kind, text }] }));
      setTimeout(() => get().dismissToast(id), kind === "error" ? 6000 : 2500);
    },
    dismissToast(id) {
      set((st) => ({ toasts: st.toasts.filter((t) => t.id !== id) }));
    },

    async checkUpdate(manual) {
      try {
        const info = await api.checkUpdate(!manual);
        if (info) set({ update: info });
        if (manual && info && !info.available) get().toast("info", `ClipVault ${info.current} is up to date`);
      } catch (e) {
        if (manual) fail(e);
      }
    },

    async installUpdate() {
      set({ updateProgress: "Starting…" });
      try {
        const msg = await api.installUpdate();
        set({ updateProgress: msg });
      } catch (e) {
        set({ updateProgress: null });
        fail(e);
      }
    },

    openView(view, id) {
      if (view === "preview") {
        const target = id ?? get().selectedId;
        if (target == null) return;
        set({ view, selectedId: target, detail: null });
        void loadDetail(target);
      } else {
        set({ view });
        if (view === "settings") void get().refreshStatus();
      }
    },
  };
});
