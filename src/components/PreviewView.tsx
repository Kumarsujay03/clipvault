import { useMemo, useState } from "react";
import DOMPurify from "dompurify";
import { ArrowLeft, ClipboardPaste, Copy, FileIcon, Pin, Star, Trash2, X } from "lucide-react";
import { useApp } from "../store";
import { api } from "../lib/api";
import { absoluteTime, formatBytes, typeLabel } from "../lib/format";
import type { ItemDetail } from "../lib/types";
import { IconButton, TitleBar } from "./PanelHeader";
import { Badge } from "./Cards";

/** Strip scripts, event handlers, forms, frames and remote loads; keep basic formatting. */
export function sanitizeHtml(html: string): string {
  return DOMPurify.sanitize(html, {
    USE_PROFILES: { html: true },
    FORBID_TAGS: ["style", "form", "input", "button", "iframe", "object", "embed", "link", "meta", "img", "video", "audio", "svg"],
    FORBID_ATTR: ["style", "srcset"],
  });
}

export function PreviewView() {
  const detail = useApp((s) => s.detail);
  const openView = useApp((s) => s.openView);
  return (
    <div className="flex min-h-0 flex-1 flex-col animate-in">
      <TitleBar title="Preview" />
      <div className="flex items-center gap-1 px-2 pb-2">
        <IconButton label="Back (Esc)" onClick={() => openView("list")}>
          <ArrowLeft className="h-4 w-4" />
        </IconButton>
        <h1 className="text-[14px] font-semibold">{detail ? typeLabel(detail) : "Preview"}</h1>
      </div>
      {detail ? <Detail item={detail} /> : <div className="flex-1" aria-busy="true" />}
    </div>
  );
}

function Detail({ item }: { item: ItemDetail }) {
  const activate = useApp((s) => s.activate);
  const togglePin = useApp((s) => s.togglePin);
  const toggleFavorite = useApp((s) => s.toggleFavorite);
  const remove = useApp((s) => s.remove);
  const [rich, setRich] = useState(false);
  const html = useMemo(() => (rich && item.richHtml ? sanitizeHtml(item.richHtml) : ""), [rich, item.richHtml]);

  return (
    <>
      <div className="flex items-center gap-1 px-3 pb-2">
        <button
          onClick={() => void activate(item.id, true)}
          className="flex h-7 items-center gap-1.5 rounded-md bg-accent px-2.5 text-[12px] font-medium text-accent-fg hover:opacity-90"
          title="Paste (Enter)"
        >
          <ClipboardPaste className="h-3.5 w-3.5" /> Paste
        </button>
        <button
          onClick={() => void activate(item.id, false)}
          className="flex h-7 items-center gap-1.5 rounded-md border border-border bg-card px-2.5 text-[12px] hover:bg-hover"
          title="Copy only (Shift+Enter)"
        >
          <Copy className="h-3.5 w-3.5" /> Copy
        </button>
        <div className="ml-auto flex">
          <IconButton label={item.isPinned ? "Unpin (Ctrl+P)" : "Pin (Ctrl+P)"} active={item.isPinned} onClick={() => void togglePin(item.id)}>
            <Pin className={`h-4 w-4 ${item.isPinned ? "fill-current" : ""}`} />
          </IconButton>
          <IconButton label={item.isFavorite ? "Remove favorite" : "Favorite"} active={item.isFavorite} onClick={() => void toggleFavorite(item.id)}>
            <Star className={`h-4 w-4 ${item.isFavorite ? "fill-current" : ""}`} />
          </IconButton>
          <IconButton label="Delete (Del)" danger onClick={() => void remove(item.id)}>
            <Trash2 className="h-4 w-4" />
          </IconButton>
        </div>
      </div>

      <div className="min-h-0 flex-1 space-y-3 overflow-y-auto px-3 pb-3">
        {item.itemType === "image" ? (
          <div className="overflow-hidden rounded-lg border border-border bg-[repeating-conic-gradient(var(--hover)_0_25%,var(--card)_0_50%)] bg-[length:14px_14px]">
            <img src={api.mediaUrl("full", item.id)} alt="Copied image" className="mx-auto max-h-[300px] object-contain" draggable={false} />
          </div>
        ) : item.itemType === "files" ? (
          <div className="space-y-1.5">
            {item.filePaths.map((p) => (
              <div key={p} className="flex items-start gap-2 rounded-lg bg-card px-3 py-2">
                <FileIcon className="mt-0.5 h-4 w-4 shrink-0 text-muted" />
                <div className="min-w-0">
                  <div className="truncate text-[12.5px] font-medium">{p.split("/").filter(Boolean).pop() ?? p}</div>
                  <div className="selectable truncate font-mono text-[11px] text-muted" title={p}>
                    {p}
                  </div>
                </div>
              </div>
            ))}
            <p className="text-[11px] text-muted">Only file locations are stored. Deleting this entry never touches the files.</p>
          </div>
        ) : (
          <>
            {item.richHtml && (
              <div role="radiogroup" aria-label="Text format" className="flex gap-1">
                {["Plain", "Formatted"].map((label, i) => (
                  <button
                    key={label}
                    role="radio"
                    aria-checked={rich === (i === 1)}
                    onClick={() => setRich(i === 1)}
                    className={`h-6 rounded-full px-2.5 text-[11.5px] ${rich === (i === 1) ? "bg-accent text-accent-fg" : "bg-card hover:bg-hover"}`}
                  >
                    {label}
                  </button>
                ))}
              </div>
            )}
            {rich && html ? (
              <div
                className="selectable prose-sm max-w-none break-words rounded-lg bg-card p-3 text-[13px] leading-relaxed [&_a]:text-accent [&_a]:underline"
                // Sanitized above; remote resources are additionally blocked by the CSP.
                dangerouslySetInnerHTML={{ __html: html }}
              />
            ) : (
              <pre
                className={`selectable whitespace-pre-wrap break-words rounded-lg bg-card p-3 leading-relaxed ${
                  item.itemType === "code" ? "font-mono text-[12px]" : "font-sans text-[13px]"
                }`}
              >
                {item.contentText}
              </pre>
            )}
            {item.textTruncated && <p className="text-[11px] text-muted">Preview truncated at 1 MB. Pasting uses the full text.</p>}
          </>
        )}

        <TagEditor item={item} />

        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-[11.5px]">
          <Meta label="Copied">{absoluteTime(item.copiedAt)}</Meta>
          {item.lastUsedAt && <Meta label="Last used">{absoluteTime(item.lastUsedAt)}</Meta>}
          {item.copyCount > 1 && <Meta label="Times copied">{item.copyCount}</Meta>}
          <Meta label="Size">
            {formatBytes(item.byteSize)}
            {item.width ? ` · ${item.width}×${item.height}px` : ""}
          </Meta>
          <Meta label="Format">{item.mimeType}</Meta>
          {item.language && <Meta label="Language">{item.language}</Meta>}
          {item.metadata.ocr === true && <Meta label="Text in image">searchable (OCR)</Meta>}
        </dl>
      </div>
    </>
  );
}

function TagEditor({ item }: { item: ItemDetail }) {
  const setTags = useApp((s) => s.setTags);
  const [draft, setDraft] = useState("");
  const add = () => {
    const t = draft.trim();
    if (!t) return;
    setDraft("");
    void setTags(item.id, [...item.tags, t]);
  };
  return (
    <div>
      <div className="mb-1 text-[11px] font-medium uppercase tracking-wide text-muted">Tags</div>
      <div className="flex flex-wrap items-center gap-1">
        {item.tags.map((t) => (
          <span key={t} className="flex items-center">
            <Badge accent>#{t}</Badge>
            <button
              aria-label={`Remove tag ${t}`}
              onClick={() => void setTags(item.id, item.tags.filter((x) => x !== t))}
              className="-ml-0.5 rounded p-0.5 text-muted hover:text-danger"
            >
              <X className="h-3 w-3" />
            </button>
          </span>
        ))}
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === ",") {
              e.preventDefault();
              e.stopPropagation();
              add();
            }
          }}
          onBlur={add}
          placeholder={item.tags.length ? "Add tag" : "Add a tag, e.g. work"}
          aria-label="Add tag"
          maxLength={32}
          className="h-6 min-w-24 flex-1 rounded border border-transparent bg-transparent px-1 text-[12px] outline-none placeholder:text-muted focus:border-border focus:bg-card"
        />
      </div>
    </div>
  );
}

function Meta({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <>
      <dt className="text-muted">{label}</dt>
      <dd className="selectable truncate">{children}</dd>
    </>
  );
}
