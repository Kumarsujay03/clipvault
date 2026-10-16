import { AlertTriangle, ClipboardList, Pin, SearchX } from "lucide-react";
import { useApp } from "../store";

type Kind = "empty" | "error";

export function EmptyState({ kind, detail }: { kind: Kind; detail?: string }) {
  const search = useApp((s) => s.search);
  const filter = useApp((s) => s.filter);
  const capture = useApp((s) => s.status?.capture);

  let icon: React.ReactNode = null;
  let title = "Nothing here";
  let body = "You'll see your clipboard history here once you've copied something.";

  if (kind === "error") {
    icon = <AlertTriangle className="h-6 w-6 text-danger" />;
    title = "Couldn't load history";
    body = detail ?? "The history database is unavailable.";
  } else if (search) {
    icon = <SearchX className="h-6 w-6 text-muted" />;
    title = "No results";
    body = `Nothing matches “${search}”.`;
  } else if (filter === "pinned") {
    icon = <Pin className="h-6 w-6 text-muted" />;
    title = "No pinned items";
    body = "Pin items you use often. Pinned items are kept until you unpin them.";
  } else if (filter !== "all") {
    icon = <ClipboardList className="h-6 w-6 text-muted" />;
    body = "Nothing of this kind has been copied yet.";
  } else if (capture?.paused) {
    body = "Recording is paused. Resume it to save what you copy.";
  }

  return (
    <div className="flex flex-1 flex-col items-center justify-center px-8 pb-10 text-center animate-in">
      {icon && <div className="mb-2">{icon}</div>}
      <div className="text-[14px] font-semibold">{title}</div>
      <p className="mt-1 max-w-[260px] text-[12.5px] leading-relaxed text-muted">{body}</p>
    </div>
  );
}
