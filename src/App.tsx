import { useEffect, useState } from "react";
import { useApp } from "./store";
import { api } from "./lib/api";
import { PanelHeader, SEARCH_INPUT_ID } from "./components/PanelHeader";
import { CardList } from "./components/Cards";
import { PreviewView } from "./components/PreviewView";
import { SettingsView } from "./components/SettingsView";
import { ConfirmDialog, Toasts } from "./components/Overlays";

const NO_WARNINGS: string[] = [];

function useTheme() {
  const theme = useApp((s) => s.settings?.theme ?? "system");
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => document.documentElement.classList.toggle("dark", theme === "dark" || (theme === "system" && mq.matches));
    apply();
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [theme]);
}

const focusSearch = (selectAll = true) => {
  const el = document.getElementById(SEARCH_INPUT_ID) as HTMLInputElement | null;
  el?.focus();
  if (selectAll) el?.select();
};

/**
 * Keyboard model (Win+V style): ↑/↓ move, Enter pastes, Shift+Enter copies only, Space
 * previews, Ctrl+P pins, Del deletes, Esc backs out and finally closes the panel.
 */
export function useKeyboard() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const st = useApp.getState();
      if (e.defaultPrevented || document.querySelector("[role=alertdialog]")) return;
      const target = e.target as HTMLElement;
      const inSearch = target.id === SEARCH_INPUT_ID;
      const inOtherField = !inSearch && /^(INPUT|SELECT|TEXTAREA)$/.test(target.tagName);
      const inMenu = !!target.closest("[role=menu]");
      const ctrl = e.ctrlKey || e.metaKey;

      if (e.key === "Escape") {
        e.preventDefault();
        if (st.view !== "list") st.openView("list");
        else if (st.search) st.setSearch("");
        else void api.hideWindow();
        return;
      }
      if (st.view === "settings" || inOtherField || inMenu) return;

      const id = st.selectedId;
      switch (e.key) {
        case "ArrowDown":
        case "ArrowUp":
          if (st.view !== "list") return;
          e.preventDefault();
          st.moveSelection(e.key === "ArrowDown" ? 1 : -1);
          return;
        case "PageDown":
        case "PageUp":
          if (st.view !== "list") return;
          e.preventDefault();
          st.moveSelection(e.key === "PageDown" ? 6 : -6);
          return;
        case "Home":
        case "End":
          if (inSearch || st.view !== "list") return;
          e.preventDefault();
          st.moveSelection(e.key === "Home" ? -1e9 : 1e9);
          return;
        case "Enter":
          if (id == null) return;
          e.preventDefault();
          void st.activate(id, !e.shiftKey);
          return;
        case " ":
          if (inSearch && st.search) return;
          if (id == null || st.view !== "list") return;
          e.preventDefault();
          st.openView("preview", id);
          return;
        case "Delete":
          if ((inSearch && st.search) || id == null) return;
          e.preventDefault();
          void st.remove(id);
          return;
      }
      if (ctrl && e.key.toLowerCase() === "p" && id != null) {
        e.preventDefault();
        void st.togglePin(id);
      } else if (ctrl && e.key.toLowerCase() === "c" && id != null && !window.getSelection()?.toString()) {
        e.preventDefault();
        void st.activate(id, false);
      } else if (ctrl && e.key.toLowerCase() === "f") {
        e.preventDefault();
        focusSearch();
      } else if (ctrl && e.key === ",") {
        e.preventDefault();
        st.openView("settings");
      } else if (!inSearch && st.view === "list" && !ctrl && !e.altKey && e.key.length === 1) {
        focusSearch(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

function ServiceBanner() {
  const status = useApp((s) => s.status);
  const setPaused = useApp((s) => s.setPaused);
  const startService = useApp((s) => s.startService);
  if (!status || api.preview) return null;
  const c = status.capture;
  let text: string | null = null;
  let action: { label: string; fn: () => void } | null = null;
  if (!c) {
    text = "Clipboard recording service isn't running.";
    action = { label: "Start", fn: () => void startService() };
  } else if (c.paused) {
    text = "Recording is paused.";
    action = { label: "Resume", fn: () => void setPaused(false) };
  } else if (c.state === "error" || c.state === "unavailable") {
    text = c.error ?? "Clipboard is unavailable.";
  }
  if (!text) return null;
  return (
    <div role="status" className="mx-3 mb-2 flex items-center gap-2 rounded-md bg-warn/15 px-2.5 py-1.5 text-[11.5px] text-warn">
      <span className="min-w-0 flex-1">{text}</span>
      {action && (
        <button onClick={action.fn} className="shrink-0 font-semibold underline-offset-2 hover:underline">
          {action.label}
        </button>
      )}
    </div>
  );
}

export default function App() {
  const init = useApp((s) => s.init);
  const view = useApp((s) => s.view);
  const shownSeq = useApp((s) => s.shownSeq);
  const clear = useApp((s) => s.clear);
  const warnings = useApp((s) => s.status?.warnings ?? NO_WARNINGS);
  const [confirmClear, setConfirmClear] = useState(false);
  useTheme();
  useKeyboard();

  useEffect(() => {
    let cleanup: (() => void) | undefined;
    let cancelled = false;
    void init().then((c) => (cancelled ? c() : (cleanup = c)));
    focusSearch();
    return () => {
      cancelled = true;
      cleanup?.();
    };
  }, [init]);

  useEffect(() => {
    if (shownSeq > 0) focusSearch();
  }, [shownSeq]);

  return (
    <div className="h-full p-2">
      <div className="relative flex h-full flex-col overflow-hidden rounded-xl border border-border bg-panel shadow-panel">
        {view === "list" && (
          <>
            <PanelHeader onClearAll={() => setConfirmClear(true)} />
            <ServiceBanner />
            {warnings.map((w) => (
              <p key={w} role="alert" className="mx-3 mb-2 rounded-md bg-warn/15 px-2.5 py-1.5 text-[11.5px] text-warn">
                {w}
              </p>
            ))}
            <CardList />
            {api.preview && (
              <div className="shrink-0 px-3 py-1.5 text-center text-[10.5px] text-warn">Preview — demo data</div>
            )}
          </>
        )}
        {view === "preview" && <PreviewView />}
        {view === "settings" && <SettingsView />}
        <Toasts />
        {confirmClear && (
          <ConfirmDialog
            title="Clear clipboard history?"
            body="All items except pinned ones will be removed."
            confirmLabel="Clear all"
            onCancel={() => setConfirmClear(false)}
            onConfirm={() => {
              setConfirmClear(false);
              void clear(false);
            }}
          />
        )}
      </div>
    </div>
  );
}
