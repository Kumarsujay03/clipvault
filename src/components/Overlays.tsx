import { useEffect, useRef } from "react";
import { useApp } from "../store";

export function Toasts() {
  const toasts = useApp((s) => s.toasts);
  const dismiss = useApp((s) => s.dismissToast);
  return (
    <div aria-live="polite" className="pointer-events-none absolute bottom-4 left-1/2 z-50 flex -translate-x-1/2 flex-col items-center gap-2">
      {toasts.map((t) => (
        <div
          key={t.id}
          role={t.kind === "error" ? "alert" : "status"}
          onClick={() => dismiss(t.id)}
          className={`pointer-events-auto w-max max-w-[300px] rounded-lg border px-3.5 py-2 text-[12.5px] shadow-panel animate-in ${
            t.kind === "error" ? "border-danger/40 bg-panel text-danger" : "border-border bg-panel text-fg"
          }`}
        >
          {t.text}
        </div>
      ))}
    </div>
  );
}

export function ConfirmDialog({
  title,
  body,
  confirmLabel,
  onConfirm,
  onCancel,
  danger = true,
}: {
  title: string;
  body: string;
  confirmLabel: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => cancelRef.current?.focus(), []);
  return (
    <div role="alertdialog" aria-modal="true" aria-label={title} className="absolute inset-0 z-[60] flex items-center justify-center rounded-xl bg-black/30 p-5">
      <div className="w-full max-w-sm rounded-xl border border-border bg-panel p-5 shadow-panel animate-in">
        <h3 className="text-[14px] font-semibold">{title}</h3>
        <p className="mt-1.5 text-[12.5px] leading-relaxed text-muted">{body}</p>
        <div className="mt-5 flex justify-end gap-2">
          <button ref={cancelRef} onClick={onCancel} className="h-8 rounded-lg border border-border px-3 text-[12.5px] hover:bg-hover">
            Cancel
          </button>
          <button onClick={onConfirm} className={`h-8 rounded-lg px-3 text-[12.5px] font-medium ${danger ? "bg-danger-solid text-white hover:bg-danger-solid/90" : "bg-accent text-accent-fg hover:opacity-90"}`}>
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
