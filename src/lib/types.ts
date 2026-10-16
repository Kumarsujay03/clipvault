// Mirrors the Rust types in src-tauri/core/src/{model,settings,capture,ipc,desktop,export}.rs
// and src-tauri/src/commands.rs.

export type ItemType = "text" | "code" | "image" | "files";

export interface ItemSummary {
  id: number;
  itemType: ItemType;
  preview: string;
  mimeType: string;
  createdAt: number;
  copiedAt: number;
  lastUsedAt: number | null;
  copyCount: number;
  isPinned: boolean;
  isFavorite: boolean;
  byteSize: number;
  width: number | null;
  height: number | null;
  hasThumbnail: boolean;
  fileCount: number;
  hasHtml: boolean;
  language: string | null;
  tags: string[];
}

export interface ItemDetail extends ItemSummary {
  contentText: string | null;
  textTruncated: boolean;
  filePaths: string[];
  sourceApplication: string | null;
  metadata: Record<string, unknown>;
  richHtml: string | null;
}

export type Filter = "all" | "pinned" | "favorites" | "text" | "code" | "image" | "files";
export type Sort = "newest" | "oldest" | "recently_used";

export interface ListQuery {
  search?: string;
  filter: Filter;
  sort: Sort;
  offset: number;
  limit: number;
}

export interface Page {
  items: ItemSummary[];
  total: number;
}

export type Counts = Record<Filter, number>;

export type Theme = "system" | "light" | "dark";
export type DedupeMode = "move_to_top" | "consecutive" | "keep_all";

export interface Settings {
  start_monitoring: boolean;
  privacy_mode_on_startup: boolean;
  theme: Theme;
  shortcut: string;
  start_at_login: boolean;
  retention_days: number;
  max_items: number;
  max_media_mb: number;
  max_item_mb: number;
  thumbnail_size: number;
  dedupe: DedupeMode;
  close_on_copy: boolean;
  capture_images: boolean;
  capture_files: boolean;
  exclude_sensitive: boolean;
  excluded_apps: string[];
  show_tray_icon: boolean;
  panel_keep_alive_secs: number;
  hide_on_blur: boolean;
  paste_on_select: boolean;
  ocr_images: boolean;
}

export type BackendState = "starting" | "running" | "error" | "unavailable" | "stopped";

export interface CaptureStatus {
  backend: string | null;
  state: BackendState;
  error: string | null;
  paused: boolean;
  session: {
    sessionType: string;
    desktop: string;
    waylandDisplay: string | null;
    xDisplay: string | null;
  };
}

export interface DaemonStatus {
  capture: CaptureStatus;
  version: string;
  pid: number;
  ocrAvailable: boolean;
  trayVisible: boolean;
}

export interface AppStatus {
  /** null when the capture service is not running. */
  capture: CaptureStatus | null;
  daemon: DaemonStatus | null;
  db: {
    ok: boolean;
    error: string | null;
    path: string;
    items: number;
    mediaBytes: number;
    diskBytes: number;
  };
  dataDir: string;
  configDir: string;
  backupDir: string;
  version: string;
  warnings: string[];
  autostart: boolean;
  isolated: boolean;
}

export interface ShortcutStatus {
  method: "extension" | "gnome" | "none";
  binding: string | null;
  extension: "active" | "pending_login" | "disabled" | "not_installed";
  message: string;
}

export interface BackupInfo {
  name: string;
  bytes: number;
  modified: number;
}

export interface ImportReport {
  imported: number;
  skipped: number;
}

export interface TagCount {
  name: string;
  count: number;
}

export interface CopyResult {
  pasted: boolean;
  hidden: boolean;
}

export type AppEvent =
  | "history-changed"
  | "status-changed"
  | "capture-skipped"
  | "backend-error"
  | "panel-shown"
  | "panel-hidden";
