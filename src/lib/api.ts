import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AppEvent,
  AppStatus,
  BackupInfo,
  CaptureStatus,
  CopyResult,
  Counts,
  ImportReport,
  ItemDetail,
  ListQuery,
  Page,
  Settings,
  ShortcutStatus,
  TagCount,
  UpdateInfo,
} from "./types";
import { createMockApi } from "./mock";

export interface Api {
  /** True when running without the Tauri backend (browser preview with demo data). */
  readonly preview: boolean;
  panelReady(): Promise<void>;
  /** Start moving the panel with the mouse (call from a mousedown handler). */
  startMove(): Promise<void>;
  /** The move started by startMove is over. */
  endMove(reason: string): Promise<void>;
  listItems(query: ListQuery): Promise<Page>;
  getItem(id: number): Promise<ItemDetail>;
  getCounts(): Promise<Counts>;
  allTags(): Promise<TagCount[]>;
  setPinned(id: number, pinned: boolean): Promise<void>;
  setFavorite(id: number, favorite: boolean): Promise<void>;
  setTags(id: number, tags: string[]): Promise<string[]>;
  deleteItem(id: number): Promise<void>;
  clearHistory(includePinned: boolean): Promise<void>;
  copyItem(id: number, paste: boolean): Promise<CopyResult>;
  getSettings(): Promise<Settings>;
  updateSettings(settings: Settings): Promise<Settings>;
  getStatus(): Promise<AppStatus>;
  getShortcutStatus(): Promise<ShortcutStatus>;
  setPaused(paused: boolean): Promise<CaptureStatus>;
  startService(): Promise<boolean>;
  hideWindow(): Promise<void>;
  quitApp(): Promise<void>;
  setupShortcut(): Promise<ShortcutStatus>;
  removeShortcut(): Promise<ShortcutStatus>;
  installExtension(): Promise<string>;
  exportHistory(includeImages: boolean): Promise<string>;
  listBackups(): Promise<BackupInfo[]>;
  importBackup(name: string): Promise<ImportReport>;
  /** Check GitHub for a newer release; with onlyIfDue, only when the daily check is due. */
  checkUpdate(onlyIfDue: boolean): Promise<UpdateInfo | null>;
  /** Download, verify and install the latest release, then restart. */
  installUpdate(): Promise<string>;
  on<T = unknown>(event: AppEvent, cb: (payload: T) => void): Promise<() => void>;
  mediaUrl(kind: "thumb" | "full", id: number): string;
}

const tauriApi: Api = {
  preview: false,
  panelReady: () => invoke("panel_ready"),
  startMove: () => invoke("start_move"),
  endMove: (reason) => invoke("end_move", { reason }),
  listItems: (query) => invoke("list_items", { query }),
  getItem: (id) => invoke("get_item", { id }),
  getCounts: () => invoke("get_counts"),
  allTags: () => invoke("all_tags"),
  setPinned: (id, pinned) => invoke("set_pinned", { id, pinned }),
  setFavorite: (id, favorite) => invoke("set_favorite", { id, favorite }),
  setTags: (id, tags) => invoke("set_tags", { id, tags }),
  deleteItem: (id) => invoke("delete_item", { id }),
  clearHistory: (includePinned) => invoke("clear_history", { includePinned }),
  copyItem: (id, paste) => invoke("copy_item", { id, paste }),
  getSettings: () => invoke("get_settings"),
  updateSettings: (settings) => invoke("update_settings", { settings }),
  getStatus: () => invoke("get_status"),
  getShortcutStatus: () => invoke("get_shortcut_status"),
  setPaused: (paused) => invoke("set_paused", { paused }),
  startService: () => invoke("start_service"),
  hideWindow: () => invoke("hide_window"),
  quitApp: () => invoke("quit_app"),
  setupShortcut: () => invoke("setup_shortcut"),
  removeShortcut: () => invoke("remove_shortcut"),
  installExtension: () => invoke("install_extension"),
  exportHistory: (includeImages) => invoke("export_history", { includeImages }),
  listBackups: () => invoke("list_backups"),
  importBackup: (name) => invoke("import_backup", { name }),
  checkUpdate: (onlyIfDue) => invoke("check_update", { onlyIfDue }),
  installUpdate: () => invoke("install_update"),
  on: (event, cb) => listen(event, (e) => cb(e.payload as never)),
  // WebKitGTK serves custom schemes as <scheme>://localhost/<path>.
  mediaUrl: (kind, id) => `cvmedia://localhost/${kind}/${id}`,
};

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const api: Api = isTauri ? tauriApi : createMockApi();

export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return "Unexpected error";
}
