import { memo, useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Eye, FileIcon, MoreHorizontal, Pin, PinOff, Star, Trash2, Copy } from "lucide-react";
import { useApp } from "../store";
import { api } from "../lib/api";
import { relativeTime } from "../lib/format";
import type { ItemSummary } from "../lib/types";
import { EmptyState } from "./EmptyState";

const estimate = (i: ItemSummary) => (i.itemType === "image" ? 132 : i.itemType === "files" ? 64 : 76);

export function CardList() {
  const items = useApp((s) => s.items);
  const total = useApp((s) => s.total);
  const selectedId = useApp((s) => s.selectedId);
  const loading = useApp((s) => s.loading);
  const loadError = useApp((s) => s.loadError);
  const loadMore = useApp((s) => s.loadMore);
  const shownSeq = useApp((s) => s.shownSeq);
  const parentRef = useRef<HTMLDivElement>(null);
  const [menuFor, setMenuFor] = useState<number | null>(null);

  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => parentRef.current,
    estimateSize: (i) => estimate(items[i]),
    getItemKey: (i) => items[i].id,
    overscan: 6,
    gap: 6,
    paddingStart: 2,
    paddingEnd: 10,
    initialRect: { width: 360, height: 420 },
  });

  const virtualItems = virtualizer.getVirtualItems();
  const last = virtualItems[virtualItems.length - 1];
  useEffect(() => {
    if (last && last.index >= items.length - 8 && items.length < total) void loadMore();
  }, [last?.index, items.length, total, loadMore]);

  useEffect(() => {
    const idx = items.findIndex((i) => i.id === selectedId);
    if (idx >= 0) virtualizer.scrollToIndex(idx, { align: "auto" });
  }, [selectedId, items, virtualizer]);

  // Opening the panel starts at the top, like Win+V.
  useEffect(() => {
    parentRef.current?.scrollTo({ top: 0 });
    setMenuFor(null);
  }, [shownSeq]);

  if (loadError) return <EmptyState kind="error" detail={loadError} />;
  if (!loading && items.length === 0) return <EmptyState kind="empty" />;

  return (
    <div
      ref={parentRef}
      role="listbox"
      aria-label="Clipboard history"
      aria-activedescendant={selectedId != null ? `item-${selectedId}` : undefined}
      tabIndex={-1}
      className="min-h-0 flex-1 overflow-y-auto px-3 outline-none"
    >
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualItems.map((v) => {
          const item = items[v.index];
          return (
            <div
              key={v.key}
              data-index={v.index}
              ref={virtualizer.measureElement}
              style={{ position: "absolute", top: 0, left: 0, right: 0, transform: `translateY(${v.start}px)` }}
            >
              <Card
                item={item}
                selected={item.id === selectedId}
                menuOpen={menuFor === item.id}
                onMenu={(open) => setMenuFor(open ? item.id : null)}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}

const Card = memo(function Card({
  item,
  selected,
  menuOpen,
  onMenu,
}: {
  item: ItemSummary;
  selected: boolean;
  menuOpen: boolean;
  onMenu: (open: boolean) => void;
}) {
  const select = useApp((s) => s.select);
  const activate = useApp((s) => s.activate);
  const togglePin = useApp((s) => s.togglePin);

  return (
    <div
      id={`item-${item.id}`}
      role="option"
      aria-selected={selected}
      title={`Copied ${relativeTime(item.copiedAt)} — click to paste`}
      onMouseEnter={() => select(item.id)}
      onClick={() => void activate(item.id, true)}
      className={`group relative cursor-default rounded-lg border bg-card px-3 py-2.5 transition-colors ${
        selected ? "border-accent/70 bg-card-hover" : "border-transparent shadow-[0_1px_2px_rgb(0_0_0/0.06)]"
      }`}
    >
      <div className="pr-14">
        <CardBody item={item} />
      </div>

      <div className="absolute right-1.5 top-1.5 flex items-center gap-0.5">
        {item.isFavorite && !selected && <Star className="m-1 h-3.5 w-3.5 fill-warn text-warn" aria-label="Favorite" />}
        <button
          aria-label={item.isPinned ? "Unpin" : "Pin"}
          title={item.isPinned ? "Unpin" : "Pin"}
          onClick={(e) => {
            e.stopPropagation();
            void togglePin(item.id);
          }}
          className={`flex h-6 w-6 items-center justify-center rounded-md hover:bg-hover ${
            item.isPinned ? "text-accent" : selected ? "text-muted" : "invisible text-muted group-hover:visible"
          }`}
        >
          <Pin className={`h-3.5 w-3.5 ${item.isPinned ? "fill-current" : ""}`} />
        </button>
        <button
          aria-label="More options"
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          onClick={(e) => {
            e.stopPropagation();
            onMenu(!menuOpen);
          }}
          className={`flex h-6 w-6 items-center justify-center rounded-md text-muted hover:bg-hover hover:text-fg ${
            selected || menuOpen ? "" : "invisible group-hover:visible"
          }`}
        >
          <MoreHorizontal className="h-4 w-4" />
        </button>
      </div>
      {menuOpen && <ItemMenu item={item} onClose={() => onMenu(false)} />}
    </div>
  );
});

function CardBody({ item }: { item: ItemSummary }) {
  if (item.itemType === "image") {
    return item.hasThumbnail ? (
      <img
        src={api.mediaUrl("thumb", item.id)}
        alt={`Copied image ${item.width}×${item.height}`}
        loading="lazy"
        draggable={false}
        className="max-h-[108px] max-w-full rounded object-contain"
      />
    ) : (
      <span className="text-muted">Image</span>
    );
  }
  if (item.itemType === "files") {
    const names = item.preview.split(", ");
    return (
      <div className="flex items-center gap-2">
        <FileIcon className="h-4 w-4 shrink-0 text-muted" />
        <span className="truncate">
          {names[0]}
          {names.length > 1 && <span className="text-muted"> +{names.length - 1} more</span>}
        </span>
      </div>
    );
  }
  const text = item.preview.trim();
  return (
    <div>
      <p
        className={`line-clamp-3 break-words leading-snug ${
          item.itemType === "code" ? "whitespace-pre-wrap font-mono text-[12px]" : "text-[13px]"
        }`}
      >
        {text || <span className="text-muted">(blank)</span>}
      </p>
      {(item.language || item.tags.length > 0) && (
        <div className="mt-1.5 flex flex-wrap gap-1">
          {item.language && <Badge>{item.language}</Badge>}
          {item.tags.map((t) => (
            <Badge key={t} accent>
              #{t}
            </Badge>
          ))}
        </div>
      )}
    </div>
  );
}

export function Badge({ children, accent }: { children: React.ReactNode; accent?: boolean }) {
  return (
    <span
      className={`rounded px-1.5 py-px text-[10.5px] ${accent ? "bg-accent-soft text-accent" : "bg-hover text-muted"}`}
    >
      {children}
    </span>
  );
}

function ItemMenu({ item, onClose }: { item: ItemSummary; onClose: () => void }) {
  const activate = useApp((s) => s.activate);
  const togglePin = useApp((s) => s.togglePin);
  const toggleFavorite = useApp((s) => s.toggleFavorite);
  const remove = useApp((s) => s.remove);
  const openView = useApp((s) => s.openView);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    ref.current?.querySelector<HTMLButtonElement>("button")?.focus();
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [onClose]);

  const entry = (label: string, icon: React.ReactNode, fn: () => void, danger = false) => (
    <button
      role="menuitem"
      onClick={(e) => {
        e.stopPropagation();
        onClose();
        fn();
      }}
      className={`flex h-8 w-full items-center gap-2.5 rounded px-2.5 text-left text-[12.5px] hover:bg-hover focus:bg-hover focus:outline-none ${
        danger ? "text-danger" : ""
      }`}
    >
      {icon}
      {label}
    </button>
  );

  return (
    <div
      ref={ref}
      role="menu"
      aria-label="Item options"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          onClose();
        }
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          e.stopPropagation();
          const btns = [...(ref.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])];
          const i = btns.indexOf(document.activeElement as HTMLButtonElement);
          btns[(i + (e.key === "ArrowDown" ? 1 : btns.length - 1)) % btns.length]?.focus();
        }
      }}
      className="absolute right-1.5 top-8 z-20 w-44 rounded-lg border border-border bg-card p-1 shadow-panel animate-in"
    >
      {entry("Copy only", <Copy className="h-3.5 w-3.5" />, () => void activate(item.id, false))}
      {entry("Preview", <Eye className="h-3.5 w-3.5" />, () => openView("preview", item.id))}
      {entry(item.isPinned ? "Unpin" : "Pin", item.isPinned ? <PinOff className="h-3.5 w-3.5" /> : <Pin className="h-3.5 w-3.5" />, () =>
        void togglePin(item.id),
      )}
      {entry(item.isFavorite ? "Remove favorite" : "Favorite", <Star className="h-3.5 w-3.5" />, () => void toggleFavorite(item.id))}
      {entry("Delete", <Trash2 className="h-3.5 w-3.5" />, () => void remove(item.id), true)}
    </div>
  );
}
