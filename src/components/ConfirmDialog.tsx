import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

export interface ConfirmOptions {
  title: string;
  description: string;
  confirmLabel?: string;
  cancelLabel?: string;
  tone?: "primary" | "destructive";
}

interface PendingConfirm extends ConfirmOptions {
  resolve: (ok: boolean) => void;
}

/**
 * 主题化二次确认弹窗：替代浏览器原生的 window.confirm，
 * 跟随当前主题的 card / border / backdrop 变量。
 */
export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel = "确定",
  cancelLabel = "取消",
  tone = "primary",
  onCancel,
  onConfirm,
}: ConfirmOptions & {
  open: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    cancelRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onCancel();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onCancel]);

  if (!open) return null;

  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-background/60 p-4 backdrop-blur-sm animate-fade-soft"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="otr-confirm-title"
        aria-describedby="otr-confirm-desc"
        data-theme-part="card"
        className="w-full max-w-md rounded-xl border border-border bg-card p-5 shadow-lg animate-pop-in"
      >
        <h3 id="otr-confirm-title" className="text-sm font-semibold text-foreground">
          {title}
        </h3>
        <p
          id="otr-confirm-desc"
          className="mt-2 max-h-60 overflow-y-auto whitespace-pre-line text-xs leading-relaxed text-muted-foreground scroll-thin"
        >
          {description}
        </p>
        <div className="mt-4 flex justify-end gap-2">
          <button
            ref={cancelRef}
            type="button"
            onClick={onCancel}
            data-theme-part="button"
            className="inline-flex h-8 items-center rounded-lg border border-border bg-background px-3 text-xs font-medium text-foreground transition-colors hover:bg-overlay/5"
          >
            {cancelLabel}
          </button>
          <button
            type="button"
            onClick={onConfirm}
            data-theme-part={tone === "destructive" ? undefined : "button-primary"}
            className={
              tone === "destructive"
                ? "inline-flex h-8 items-center rounded-lg bg-destructive px-3 text-xs font-medium text-destructive-foreground transition-colors hover:bg-destructive/90"
                : "inline-flex h-8 items-center rounded-lg bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90"
            }
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

/**
 * Promise 化的确认弹窗 hook，方便异步流程 await confirm(...)。
 */
export function useConfirm() {
  const [pending, setPending] = useState<PendingConfirm | null>(null);
  const pendingRef = useRef<PendingConfirm | null>(null);
  pendingRef.current = pending;

  useEffect(() => {
    return () => {
      pendingRef.current?.resolve(false);
    };
  }, []);

  const confirm = useCallback((options: ConfirmOptions): Promise<boolean> => {
    return new Promise((resolve) => {
      pendingRef.current?.resolve(false);
      setPending({ ...options, resolve });
    });
  }, []);

  const close = useCallback((ok: boolean) => {
    setPending((cur) => {
      cur?.resolve(ok);
      return null;
    });
  }, []);

  const dialog = (
    <ConfirmDialog
      open={pending !== null}
      title={pending?.title ?? ""}
      description={pending?.description ?? ""}
      confirmLabel={pending?.confirmLabel}
      cancelLabel={pending?.cancelLabel}
      tone={pending?.tone}
      onCancel={() => close(false)}
      onConfirm={() => close(true)}
    />
  );

  return { confirm, dialog };
}
