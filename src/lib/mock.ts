// In-memory backend for browser preview and tests ONLY. The UI shows a "Preview — demo data"
// label whenever this is active; the desktop app never uses it.
import type { Api } from "./api";
import type { AppStatus, Counts, ItemDetail, ItemType, ListQuery, Settings, ShortcutStatus } from "./types";

export const defaultSettings: Settings = {
  start_monitoring: true,
  privacy_mode_on_startup: false,
  theme: "system",
  shortcut: "Super+V",
  start_at_login: true,
  retention_days: 30,
  max_items: 1000,
  max_media_mb: 500,
  max_item_mb: 25,
  thumbnail_size: 320,
  dedupe: "move_to_top",
  close_on_copy: true,
  capture_images: true,
  capture_files: true,
  exclude_sensitive: true,
  excluded_apps: [],
  show_tray_icon: true,
  panel_keep_alive_secs: 120,
  hide_on_blur: true,
  paste_on_select: true,
  ocr_images: false,
  check_updates: false,
};

const DEMO_IMAGE =
  "data:image/svg+xml;utf8," +
  encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180"><defs><linearGradient id="g" x2="1" y2="1"><stop offset="0" stop-color="#6366f1"/><stop offset="1" stop-color="#0ea5e9"/></linearGradient></defs><rect width="320" height="180" fill="url(#g)"/><text x="160" y="98" font-family="sans-serif" font-size="18" fill="white" text-anchor="middle">demo image</text></svg>`,
  );

function make(id: number, itemType: ItemType, text: string, minutesAgo: number, extra: Partial<ItemDetail> = {}): ItemDetail {
  const t = Date.now() - minutesAgo * 60_000;
  return {
    id,
    itemType,
    preview: text.slice(0, 500),
    mimeType: itemType === "image" ? "image/png" : itemType === "files" ? "text/uri-list" : "text/plain",
    createdAt: t,
    copiedAt: t,
    lastUsedAt: null,
    copyCount: 1,
    isPinned: false,
    isFavorite: false,
    byteSize: text.length,
    width: null,
    height: null,
    hasThumbnail: itemType === "image",
    fileCount: 0,
    hasHtml: false,
    language: null,
    tags: [],
    contentText: itemType === "image" ? null : text,
    textTruncated: false,
    filePaths: [],
    sourceApplication: null,
    metadata: {},
    richHtml: null,
    ...extra,
  };
}

export function demoItems(): ItemDetail[] {
  return [
    make(1, "text", "Demo: meeting notes — ship the release candidate on Thursday.", 2, {
      hasHtml: true,
      richHtml: "<p><b>Demo:</b> meeting notes — <i>ship the release candidate</i> on Thursday.</p>",
    }),
    make(2, "code", 'fn main() {\n    let vault = ClipVault::open()?;\n    println!("{} items", vault.len());\n}', 5, {
      language: "rust",
    }),
    make(3, "image", "Image 320×180", 12, { width: 320, height: 180, byteSize: 48_213 }),
    make(4, "files", "report.pdf, screenshot.png", 30, {
      fileCount: 2,
      filePaths: ["/home/demo/Documents/report.pdf", "/home/demo/Pictures/screenshot.png"],
      contentText: "/home/demo/Documents/report.pdf\n/home/demo/Pictures/screenshot.png",
    }),
    make(5, "text", "https://example.com/demo-link", 90, { isPinned: true, tags: ["work"] }),
    make(6, "code", "sudo apt install libwebkit2gtk-4.1-dev librsvg2-dev", 300, { isFavorite: true, language: "shell" }),
  ];
}

export function createMockApi(seed: ItemDetail[] = demoItems()): Api {
  let items = seed.map((i) => ({ ...i }));
  let settings = { ...defaultSettings };
  let paused = false;
  let shortcut: ShortcutStatus = { method: "none", binding: null, extension: "not_installed", message: "No keyboard shortcut is registered yet." };
  const listeners = new Map<string, Set<(p: unknown) => void>>();
  const emit = (name: string, payload?: unknown) => listeners.get(name)?.forEach((cb) => cb(payload));
  const find = (id: number) => {
    const it = items.find((i) => i.id === id);
    if (!it) throw "not found";
    return it;
  };
  const capture = () => ({
    backend: "preview",
    state: "running" as const,
    error: null,
    paused,
    session: { sessionType: "preview", desktop: "browser", waylandDisplay: null, xDisplay: null },
  });

  const status = (): AppStatus => ({
    capture: capture(),
    daemon: { capture: capture(), version: "preview", pid: 0, ocrAvailable: false, trayVisible: settings.show_tray_icon },
    db: { ok: true, error: null, path: "(in memory)", items: items.length, mediaBytes: 0, diskBytes: 0 },
    dataDir: "(preview)",
    configDir: "(preview)",
    backupDir: "(preview)",
    version: "preview",
    warnings: [],
    autostart: settings.start_at_login,
    isolated: true,
  });

  return {
    preview: true,
    async panelReady() {},
    async startMove() {},
    async endMove() {},
    async listItems(q: ListQuery) {
      const terms = (q.search ?? "").toLowerCase().split(/\s+/).filter(Boolean);
      let rows = items.filter((i) => {
        if (q.filter === "pinned" && !i.isPinned) return false;
        if (q.filter === "favorites" && !i.isFavorite) return false;
        if (["text", "code", "image", "files"].includes(q.filter) && i.itemType !== q.filter) return false;
        const hay = `${i.contentText ?? ""} ${i.preview} ${i.tags.map((t) => "#" + t).join(" ")}`.toLowerCase();
        return terms.every((t) => hay.includes(t));
      });
      const key = (i: ItemDetail) => (q.sort === "recently_used" ? (i.lastUsedAt ?? i.copiedAt) : i.copiedAt);
      rows = rows.sort((a, b) => (q.sort === "oldest" ? key(a) - key(b) : key(b) - key(a)));
      return { total: rows.length, items: rows.slice(q.offset, q.offset + (q.limit || 100)) };
    },
    async getItem(id) {
      return { ...find(id) };
    },
    async getCounts(): Promise<Counts> {
      const c = (f: (i: ItemDetail) => boolean) => items.filter(f).length;
      return {
        all: items.length,
        pinned: c((i) => i.isPinned),
        favorites: c((i) => i.isFavorite),
        text: c((i) => i.itemType === "text"),
        code: c((i) => i.itemType === "code"),
        image: c((i) => i.itemType === "image"),
        files: c((i) => i.itemType === "files"),
      };
    },
    async allTags() {
      const m = new Map<string, number>();
      items.forEach((i) => i.tags.forEach((t) => m.set(t, (m.get(t) ?? 0) + 1)));
      return [...m].map(([name, count]) => ({ name, count }));
    },
    async setPinned(id, pinned) {
      find(id).isPinned = pinned;
      emit("history-changed");
    },
    async setFavorite(id, fav) {
      find(id).isFavorite = fav;
      emit("history-changed");
    },
    async setTags(id, tags) {
      const clean = [...new Set(tags.map((t) => t.trim().replace(/^#/, "").toLowerCase().replace(/\s+/g, "-")).filter(Boolean))];
      find(id).tags = clean;
      emit("history-changed");
      return clean;
    },
    async deleteItem(id) {
      find(id);
      items = items.filter((i) => i.id !== id);
      emit("history-changed");
    },
    async clearHistory(includePinned) {
      items = items.filter((i) => !includePinned && i.isPinned);
      emit("history-changed");
    },
    async copyItem(id) {
      find(id).lastUsedAt = Date.now();
      emit("history-changed");
      return { pasted: false, hidden: false };
    },
    async getSettings() {
      return { ...settings };
    },
    async updateSettings(s) {
      settings = { ...s };
      return { ...settings };
    },
    async getStatus() {
      return status();
    },
    async getShortcutStatus() {
      return shortcut;
    },
    async setPaused(p) {
      paused = p;
      emit("status-changed", capture());
      return capture();
    },
    async startService() {
      return true;
    },
    async hideWindow() {},
    async quitApp() {},
    async setupShortcut() {
      shortcut = { method: "gnome", binding: "Super+V", extension: shortcut.extension, message: "Registered as a GNOME keyboard shortcut (Super+V)." };
      return shortcut;
    },
    async removeShortcut() {
      shortcut = { ...shortcut, method: "none", binding: null, message: "No keyboard shortcut is registered yet." };
      return shortcut;
    },
    async installExtension() {
      shortcut = { ...shortcut, extension: "pending_login" };
      return "pending_login";
    },
    async checkUpdate() {
      return { current: "preview", latest: "preview", available: false, notes: "", url: "", publishedAt: "", installKind: "manual", assetSize: null };
    },
    async installUpdate() {
      return "Updates are not available in preview mode.";
    },
    async exportHistory() {
      return "(preview)/clipvault-demo.json";
    },
    async listBackups() {
      return [];
    },
    async importBackup() {
      return { imported: 0, skipped: 0 };
    },
    async on(event, cb) {
      if (!listeners.has(event)) listeners.set(event, new Set());
      listeners.get(event)!.add(cb as (p: unknown) => void);
      return () => listeners.get(event)?.delete(cb as (p: unknown) => void);
    },
    mediaUrl: () => DEMO_IMAGE,
  };
}
