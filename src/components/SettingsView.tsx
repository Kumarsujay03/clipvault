import { useCallback, useEffect, useState } from "react";
import { ArrowLeft } from "lucide-react";
import { useApp } from "../store";
import { api, errorMessage } from "../lib/api";
import { absoluteTime, formatBytes } from "../lib/format";
import type { BackupInfo, DedupeMode, Settings, ShortcutStatus, Theme } from "../lib/types";
import { IconButton, TitleBar } from "./PanelHeader";
import { ConfirmDialog } from "./Overlays";

type NumKey = "retention_days" | "max_items" | "max_media_mb" | "max_item_mb" | "thumbnail_size";

// Must match Settings::validate in src-tauri/core/src/settings.rs.
export const LIMITS: Record<NumKey, [number, number]> = {
  retention_days: [0, 3650],
  max_items: [10, 100000],
  max_media_mb: [10, 100000],
  max_item_mb: [1, 512],
  thumbnail_size: [96, 1024],
};

export function validate(s: Settings): Partial<Record<NumKey, string>> {
  const errors: Partial<Record<NumKey, string>> = {};
  for (const [k, [lo, hi]] of Object.entries(LIMITS) as [NumKey, [number, number]][]) {
    const v = s[k];
    if (!Number.isInteger(v) || v < lo || v > hi) errors[k] = `${lo}–${hi}`;
  }
  return errors;
}

const KEEP_ALIVE: { v: number; label: string }[] = [
  { v: 0, label: "Unload immediately (least memory)" },
  { v: 30, label: "30 seconds" },
  { v: 120, label: "2 minutes" },
  { v: 600, label: "10 minutes" },
  { v: 3600, label: "1 hour (fastest reopen)" },
];

export function SettingsView() {
  const settings = useApp((s) => s.settings);
  const status = useApp((s) => s.status);
  const save = useApp((s) => s.saveSettings);
  const clear = useApp((s) => s.clear);
  const toast = useApp((s) => s.toast);
  const openView = useApp((s) => s.openView);
  const [draft, setDraft] = useState<Settings | null>(settings);
  const [shortcut, setShortcut] = useState<ShortcutStatus | null>(null);
  const [backups, setBackups] = useState<BackupInfo[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<null | "unpinned" | "all" | { restore: string }>(null);
  const [includeImages, setIncludeImages] = useState(true);

  useEffect(() => setDraft(settings), [settings]);
  const loadExtras = useCallback(() => {
    api.getShortcutStatus().then(setShortcut, () => setShortcut(null));
    api.listBackups().then(setBackups, () => setBackups([]));
  }, []);
  useEffect(loadExtras, [loadExtras]);

  if (!draft) return null;
  const errors = validate(draft);

  const commit = (next: Settings) => {
    setDraft(next);
    if (Object.keys(validate(next)).length === 0) void save(next);
  };
  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => commit({ ...draft, [k]: v });

  const run = async (label: string, fn: () => Promise<unknown>) => {
    setBusy(label);
    try {
      await fn();
    } catch (e) {
      toast("error", errorMessage(e));
    } finally {
      setBusy(null);
      loadExtras();
    }
  };

  const toggle = (k: keyof Settings, label: string, hint?: string, disabled?: boolean) => (
    <Row label={label} hint={hint}>
      <input
        type="checkbox"
        role="switch"
        disabled={disabled}
        checked={draft[k] as boolean}
        onChange={(e) => set(k, e.target.checked as never)}
        className="h-4 w-4 accent-[var(--accent)]"
      />
    </Row>
  );

  const num = (k: NumKey, label: string, hint?: string) => (
    <Row label={label} hint={errors[k] ? `Allowed: ${errors[k]}` : hint} error={!!errors[k]}>
      <input
        type="number"
        min={LIMITS[k][0]}
        max={LIMITS[k][1]}
        value={Number.isNaN(draft[k]) ? "" : draft[k]}
        onChange={(e) => setDraft({ ...draft, [k]: e.target.value === "" ? NaN : Number(e.target.value) })}
        onBlur={() => commit(draft)}
        onKeyDown={(e) => e.key === "Enter" && commit(draft)}
        aria-invalid={!!errors[k]}
        aria-label={label}
        className={`h-7 w-20 rounded-md border bg-card px-1.5 text-right tabular-nums outline-none focus:border-accent ${
          errors[k] ? "border-danger" : "border-border"
        }`}
      />
    </Row>
  );

  const select = <T extends string | number>(label: string, value: T, options: { v: T; label: string }[], onChange: (v: T) => void) => (
    <Row label={label}>
      <select
        value={String(value)}
        aria-label={label}
        onChange={(e) => {
          const found = options.find((o) => String(o.v) === e.target.value);
          if (found) onChange(found.v);
        }}
        className="h-7 max-w-40 rounded-md border border-border bg-card px-1.5 text-[12px] outline-none focus:border-accent"
      >
        {options.map((o) => (
          <option key={String(o.v)} value={String(o.v)}>
            {o.label}
          </option>
        ))}
      </select>
    </Row>
  );

  const ext = shortcut?.extension;
  const ocr = status?.daemon?.ocrAvailable ?? false;

  return (
    <div className="flex min-h-0 flex-1 flex-col animate-in">
      <TitleBar title="Settings" />
      <div className="flex items-center gap-1 px-2 pb-2">
        <IconButton label="Back (Esc)" onClick={() => openView("list")}>
          <ArrowLeft className="h-4 w-4" />
        </IconButton>
        <h1 className="text-[14px] font-semibold">Settings</h1>
      </div>

      <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-3 pb-4">
        {status?.isolated && !api.preview && (
          <p className="rounded-md bg-warn/15 px-2.5 py-1.5 text-[11.5px] text-warn">
            Test profile: start-at-login and keyboard shortcut changes are disabled.
          </p>
        )}

        <Section title="Keyboard shortcut">
          <p className="text-[12px] leading-relaxed text-muted">{shortcut?.message ?? "Checking…"}</p>
          <div className="flex flex-wrap gap-1.5">
            {shortcut?.method !== "extension" && (
              <Button busy={busy === "sc"} onClick={() => run("sc", api.setupShortcut)}>
                {shortcut?.method === "gnome" ? "Re-apply" : `Use ${draft.shortcut}`}
              </Button>
            )}
            {shortcut?.method === "gnome" && (
              <Button busy={busy === "rm"} onClick={() => run("rm", api.removeShortcut)}>
                Remove
              </Button>
            )}
          </div>
          <Row label="GNOME extension" hint={extensionHint(ext)}>
            {ext !== "active" && ext !== "pending_login" && (
              <Button busy={busy === "ext"} onClick={() => run("ext", api.installExtension)}>
                Install
              </Button>
            )}
          </Row>
          {toggle(
            "paste_on_select",
            "Paste after choosing an item",
            ext === "active" ? "Pastes into the window you were using" : "Needs the GNOME extension; otherwise press Ctrl+V yourself",
          )}
        </Section>

        <Section title="General">
          {toggle("start_at_login", "Start at login", "Only the small capture service starts; the panel loads when you open it")}
          {toggle("show_tray_icon", "Show icon in the top bar")}
          {toggle("hide_on_blur", "Close when clicking elsewhere")}
          {select<Theme>("Theme", draft.theme, [
            { v: "system", label: "Follow system" },
            { v: "light", label: "Light" },
            { v: "dark", label: "Dark" },
          ], (v) => set("theme", v))}
        </Section>

        <Section title="Performance">
          {select<number>("Keep panel loaded after closing", draft.panel_keep_alive_secs, KEEP_ALIVE.some((o) => o.v === draft.panel_keep_alive_secs) ? KEEP_ALIVE : [...KEEP_ALIVE, { v: draft.panel_keep_alive_secs, label: `${draft.panel_keep_alive_secs} s` }], (v) =>
            set("panel_keep_alive_secs", v),
          )}
          <p className="text-[11px] leading-relaxed text-muted">
            While closed, only the capture service runs
            {status?.daemon ? ` (pid ${status.daemon.pid})` : ""}. It uses a few MB of memory and no CPU while idle.
          </p>
        </Section>

        <Section title="Capture">
          {toggle("start_monitoring", "Record clipboard when ClipVault starts")}
          {toggle("privacy_mode_on_startup", "Privacy mode", "Recording stays off after login until you resume it")}
          {toggle("capture_images", "Images and screenshots")}
          {toggle("capture_files", "Copied files", "Stores file locations only")}
          {toggle("exclude_sensitive", "Skip likely passwords and keys", "Best effort, not a guarantee")}
          {toggle("ocr_images", "Make text in images searchable", ocr ? "Runs Tesseract locally at low priority" : "Install tesseract-ocr to enable", !ocr && !draft.ocr_images)}
          {select<DedupeMode>("Repeated copies", draft.dedupe, [
            { v: "move_to_top", label: "Move to top" },
            { v: "consecutive", label: "Merge back-to-back" },
            { v: "keep_all", label: "Keep every copy" },
          ], (v) => set("dedupe", v))}
          {num("max_item_mb", "Largest item (MB)")}
        </Section>

        <Section title="History">
          {num("retention_days", "Keep items for (days)", "0 = until the item limit")}
          {num("max_items", "Maximum items")}
          {num("max_media_mb", "Image storage limit (MB)")}
          {num("thumbnail_size", "Thumbnail size (px)")}
          <p className="text-[11px] text-muted">Pinned items are never removed automatically.</p>
          {status && (
            <p className="text-[11px] text-muted">
              {status.db.items} items · {formatBytes(status.db.diskBytes)} of images · <span className="selectable">{status.dataDir}</span>
            </p>
          )}
          <div className="flex gap-1.5">
            <Button onClick={() => setConfirm("unpinned")}>Clear unpinned</Button>
            <Button danger onClick={() => setConfirm("all")}>
              Clear everything
            </Button>
          </div>
        </Section>

        <Section title="Backup">
          <label className="flex items-center gap-2 text-[12px]">
            <input type="checkbox" checked={includeImages} onChange={(e) => setIncludeImages(e.target.checked)} className="accent-[var(--accent)]" />
            Include images
          </label>
          <Button
            busy={busy === "export"}
            onClick={() =>
              run("export", async () => {
                const path = await api.exportHistory(includeImages);
                toast("info", `Saved ${path.split("/").pop()}`);
              })
            }
          >
            Export history
          </Button>
          {backups.length > 0 && (
            <ul className="space-y-1">
              {backups.slice(0, 5).map((b) => (
                <li key={b.name} className="flex items-center gap-2 rounded-md bg-card px-2 py-1.5 text-[11.5px]">
                  <span className="min-w-0 flex-1 truncate" title={b.name}>
                    {absoluteTime(b.modified)} · {formatBytes(b.bytes)}
                  </span>
                  <button className="text-accent hover:underline" onClick={() => setConfirm({ restore: b.name })}>
                    Restore
                  </button>
                </li>
              ))}
            </ul>
          )}
          {status && <p className="selectable text-[11px] text-muted">Folder: {status.backupDir}</p>}
        </Section>

        <Section title="About">
          <p className="text-[11.5px] text-muted">
            ClipVault {status?.version} · everything stays on this computer · no network access
          </p>
          {status?.capture && (
            <p className="text-[11px] text-muted">
              Clipboard via {status.capture.backend === "xfixes" ? "Xwayland/X11" : status.capture.backend} · {status.capture.session.sessionType} ·{" "}
              {status.capture.session.desktop}
            </p>
          )}
          <Button danger onClick={() => void api.quitApp()}>
            Quit ClipVault completely
          </Button>
        </Section>
      </div>

      {confirm && (
        <ConfirmDialog
          title={typeof confirm === "object" ? "Restore this backup?" : confirm === "all" ? "Clear all history?" : "Clear unpinned history?"}
          body={
            typeof confirm === "object"
              ? "Items from the backup are added to your history. Items you already have are skipped."
              : confirm === "all"
                ? "Every item, including pinned ones, will be removed. Files on disk are not affected."
                : "All unpinned items will be removed. Pinned items stay. Files on disk are not affected."
          }
          confirmLabel={typeof confirm === "object" ? "Restore" : "Clear"}
          danger={typeof confirm !== "object"}
          onCancel={() => setConfirm(null)}
          onConfirm={() => {
            const c = confirm;
            setConfirm(null);
            if (typeof c === "object") {
              void run("import", async () => {
                const r = await api.importBackup(c.restore);
                toast("info", `Restored ${r.imported} items (${r.skipped} already present)`);
              });
            } else void clear(c === "all");
          }}
        />
      )}
    </div>
  );
}

function extensionHint(state: ShortcutStatus["extension"] | undefined) {
  switch (state) {
    case "active":
      return "Active — opens at the pointer and pastes for you";
    case "pending_login":
      return "Installed — log out and back in to activate";
    case "disabled":
      return "Installed but disabled";
    default:
      return "Optional: opens the panel at the pointer and pastes automatically";
  }
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="space-y-2 rounded-lg bg-card/60 p-3">
      <h2 className="text-[11px] font-semibold uppercase tracking-wide text-muted">{title}</h2>
      {children}
    </section>
  );
}

function Row({ label, hint, error, children }: { label: string; hint?: string; error?: boolean; children?: React.ReactNode }) {
  return (
    <label className="flex items-center justify-between gap-3 text-[12.5px]">
      <span className="min-w-0">
        <span className="block">{label}</span>
        {hint && <span className={`block text-[11px] leading-snug ${error ? "text-danger" : "text-muted"}`}>{hint}</span>}
      </span>
      <span className="shrink-0">{children}</span>
    </label>
  );
}

function Button({ children, onClick, busy, danger }: { children: React.ReactNode; onClick: () => void; busy?: boolean; danger?: boolean }) {
  return (
    <button
      onClick={onClick}
      disabled={busy}
      className={`h-7 rounded-md border px-2.5 text-[12px] disabled:opacity-50 ${
        danger ? "border-danger/50 text-danger hover:bg-danger/10" : "border-border bg-card hover:bg-hover"
      }`}
    >
      {busy ? "Working…" : children}
    </button>
  );
}
