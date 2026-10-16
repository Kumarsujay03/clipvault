import { useEffect, useRef, useState } from "react";
import { Search, Settings, X } from "lucide-react";
import { useApp } from "../store";
import { api } from "../lib/api";
import type { Filter } from "../lib/types";

export const SEARCH_INPUT_ID = "clipvault-search";

// Moving the panel. GNOME takes focus away from a window while it moves it, so the backend
// must know when a move starts and ends. It ends with the button release, or (when the
// compositor swallowed the release during the move) the first pointer motion with no button
// held. A press-and-hold before moving keeps the button down, so it never ends the move early.
let moving = false;
function endMove(reason: string) {
  if (!moving) return;
  moving = false;
  void api.endMove(reason);
}
if (typeof window !== "undefined") {
  window.addEventListener("mouseup", () => endMove("mouseup"), true);
  window.addEventListener("mousemove", (e) => e.buttons === 0 && endMove("motion without button"), true);
}

const CHIPS: { id: Filter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "pinned", label: "Pinned" },
  { id: "text", label: "Text" },
  { id: "image", label: "Images" },
  { id: "code", label: "Code" },
  { id: "files", label: "Files" },
  { id: "favorites", label: "Favorites" },
];

/** Drag handle + title row + search + filter chips. */
export function PanelHeader({ onClearAll }: { onClearAll: () => void }) {
  const search = useApp((s) => s.search);
  const setSearch = useApp((s) => s.setSearch);
  const filter = useApp((s) => s.filter);
  const setFilter = useApp((s) => s.setFilter);
  const counts = useApp((s) => s.counts);
  const openView = useApp((s) => s.openView);
  const [draft, setDraft] = useState(search);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => setDraft(search), [search]);

  const onChange = (v: string) => {
    setDraft(v);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setSearch(v), 100);
  };

  return (
    <div className="shrink-0">
      <TitleBar title="Clipboard">
        <IconButton label="Settings" onClick={() => openView("settings")}>
          <Settings className="h-4 w-4" />
        </IconButton>
      </TitleBar>

      <div className="flex items-center justify-between px-4 pb-2">
        <h1 className="text-[14px] font-semibold">Clipboard</h1>
        {(counts?.all ?? 0) > (counts?.pinned ?? 0) && (
          <button onClick={onClearAll} className="rounded-md px-2 py-1 text-[12px] text-accent hover:bg-hover">
            Clear all
          </button>
        )}
      </div>

      <div className="relative mx-3 mb-2">
        <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted" />
        <input
          id={SEARCH_INPUT_ID}
          type="search"
          value={draft}
          onChange={(e) => onChange(e.target.value)}
          placeholder="Search"
          aria-label="Search clipboard history"
          autoComplete="off"
          spellCheck={false}
          className="h-8 w-full rounded-md border border-border bg-card pl-8 pr-7 text-[13px] outline-none placeholder:text-muted focus:border-accent [&::-webkit-search-cancel-button]:hidden"
        />
        {draft && (
          <button
            aria-label="Clear search"
            onClick={() => {
              setDraft("");
              setSearch("");
            }}
            className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-1 text-muted hover:bg-hover hover:text-fg"
          >
            <X className="h-3 w-3" />
          </button>
        )}
      </div>

      <div role="tablist" aria-label="Filter" className="flex gap-1 overflow-x-auto px-3 pb-2 [scrollbar-width:none]">
        {CHIPS.map((c) => {
          const active = filter === c.id;
          const n = counts?.[c.id] ?? 0;
          if (c.id !== "all" && c.id !== "pinned" && n === 0 && !active) return null;
          return (
            <button
              key={c.id}
              role="tab"
              aria-selected={active}
              onClick={() => setFilter(c.id)}
              className={`h-6 shrink-0 rounded-full px-2.5 text-[11.5px] transition-colors ${
                active ? "bg-accent font-medium text-accent-fg" : "bg-card text-fg/80 hover:bg-hover"
              }`}
            >
              {c.label}
            </button>
          );
        })}
      </div>
    </div>
  );
}

/** The top strip: drag handle in the middle, buttons + close on the right. */
export function TitleBar({ title, children }: { title: string; children?: React.ReactNode }) {
  return (
    <div
      className="relative flex h-9 cursor-grab items-center justify-end gap-0.5 px-2 active:cursor-grabbing"
      title={title}
      onMouseDown={(e) => {
        // Drag anywhere on the strip except its buttons.
        if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
        e.preventDefault();
        moving = true;
        void api.startMove();
      }}
    >
      <div className="pointer-events-none absolute left-1/2 top-2 h-1 w-9 -translate-x-1/2 rounded-full bg-muted/40" />
      {children}
      <IconButton label="Close" onClick={() => void api.hideWindow()}>
        <X className="h-4 w-4" />
      </IconButton>
    </div>
  );
}

export function IconButton({
  label,
  onClick,
  children,
  active,
  danger,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
  active?: boolean;
  danger?: boolean;
}) {
  return (
    <button
      aria-label={label}
      title={label}
      aria-pressed={active}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      className={`flex h-7 w-7 items-center justify-center rounded-md transition-colors hover:bg-hover ${
        danger ? "text-muted hover:text-danger" : active ? "text-accent" : "text-muted hover:text-fg"
      }`}
    >
      {children}
    </button>
  );
}
