import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { PriceEntry } from "../../api/bindings";
import { ChevronDownIcon } from "../icons";

export interface ModelsDevCandidate {
  provider: string;
  entry: PriceEntry;
  official?: boolean;
}

type PopoverPos = {
  left: number;
  width: number;
  maxHeight: number;
  top?: number;
  bottom?: number;
};

function fmtRate(n: number): string {
  if (n === 0) return "0";
  if (n >= 1) return Number(n.toFixed(2)).toString();
  return Number(n.toFixed(4)).toString();
}

/**
 * 成本定价表模型行下的「供应商切换胶囊 + 浮层」：
 * 展示 models.dev 匹配到的所有供应商及其 入/出 $/M 单价，选中即更新该模型定价。
 */
export function ModelProviderPicker({
  model,
  label,
  tone,
  selectedProvider,
  candidates,
  onPick,
}: {
  model: string;
  label: string;
  tone: "primary" | "warning" | "muted";
  selectedProvider: string | null;
  candidates: readonly ModelsDevCandidate[];
  onPick: (candidate: ModelsDevCandidate) => void;
}) {
  const listId = useId();
  const btnRef = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<PopoverPos | null>(null);

  const measure = () => {
    const el = btnRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const margin = 8;
    const gap = 6;
    const spaceBelow = window.innerHeight - r.bottom - gap - margin;
    const spaceAbove = r.top - gap - margin;
    const flip = spaceBelow < 180 && spaceAbove > spaceBelow;
    const maxHeight = Math.max(120, Math.min(280, flip ? spaceAbove : spaceBelow));
    const width = Math.min(320, window.innerWidth - margin * 2);
    let left = r.left;
    if (left + width > window.innerWidth - margin) {
      left = Math.max(margin, window.innerWidth - margin - width);
    }
    setPos({
      left,
      width,
      maxHeight,
      top: flip ? undefined : r.bottom + gap,
      bottom: flip ? window.innerHeight - r.top + gap : undefined,
    });
  };

  useLayoutEffect(() => {
    if (!open) {
      setPos(null);
      return;
    }
    measure();
    const onScroll = (e: Event) => {
      const node = e.target;
      if (node instanceof Node && document.getElementById(listId)?.contains(node)) return;
      measure();
    };
    const onBlur = () => setOpen(false);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", measure);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", measure);
      window.removeEventListener("blur", onBlur);
    };
  }, [open, listId]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target;
      if (!(t instanceof Node)) return;
      if (btnRef.current?.contains(t)) return;
      const menu = document.getElementById(listId);
      if (menu?.contains(t)) return;
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        setOpen(false);
        btnRef.current?.focus();
      } else if (e.key === "Tab") {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, listId]);

  const pillTone =
    tone === "warning"
      ? open
        ? "border-warning bg-warning/20 text-warning-label"
        : "border-warning/30 bg-warning/10 text-warning-label hover:bg-warning/20"
      : open
        ? "border-primary bg-primary/20 text-primary"
        : "border-primary/30 bg-primary/10 text-primary hover:bg-primary/20";

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        title={`点击切换 ${model} 的 models.dev 定价供应商（共 ${candidates.length} 家）`}
        className={`mt-1 inline-flex max-w-full items-center gap-1 rounded-md border px-1.5 py-0.5 text-10px font-medium transition-colors ${pillTone}`}
      >
        <span className="truncate">{label}</span>
        <ChevronDownIcon
          className={`h-3 w-3 shrink-0 transition-transform duration-150 ${
            open ? "rotate-180" : ""
          }`}
        />
      </button>

      {open && pos
        ? createPortal(
            <div
              id={listId}
              role="listbox"
              aria-label={`选择 ${model} 的定价来源`}
              style={{
                position: "fixed",
                left: pos.left,
                width: pos.width,
                maxHeight: pos.maxHeight,
                top: pos.top,
                bottom: pos.bottom,
                zIndex: 70,
              }}
              className="flex flex-col overflow-hidden rounded-xl border border-border bg-popover text-popover-foreground shadow-lg animate-pop-in origin-top"
            >
              <div className="flex shrink-0 items-center justify-between gap-2 border-b border-border/60 px-3 py-2 text-11px text-muted-foreground">
                <span className="min-w-0 truncate" title={model}>
                  选择 <span className="font-medium text-foreground">{model}</span> 的来源
                </span>
                <span className="shrink-0">$/M tokens</span>
              </div>
              <div className="overflow-y-auto p-1 scroll-thin">
                {candidates.map((cand) => {
                  const selected = selectedProvider === cand.provider;
                  const detailTip = `输入 $${cand.entry.input}/M · 输出 $${cand.entry.output}/M · 缓存读 $${cand.entry.cacheRead}/M · 缓存写 $${cand.entry.cacheWrite}/M`;
                  return (
                    <button
                      key={cand.provider}
                      type="button"
                      role="option"
                      aria-selected={selected}
                      title={detailTip}
                      onClick={() => {
                        onPick(cand);
                        setOpen(false);
                        btnRef.current?.focus();
                      }}
                      className={`flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs transition-colors ${
                        selected
                          ? "bg-primary/15 font-medium text-foreground"
                          : "text-popover-foreground hover:bg-overlay/5"
                      }`}
                    >
                      <span className="flex min-w-0 items-center gap-1.5">
                        <span className="truncate">{cand.provider}</span>
                        {cand.official ? (
                          <span
                            data-theme-part="badge"
                            className="shrink-0 rounded bg-success/15 px-1 py-0.5 text-10px font-medium text-success-text"
                          >
                            官方
                          </span>
                        ) : null}
                      </span>
                      <span className="flex shrink-0 items-center gap-2 text-11px tabular-nums text-muted-foreground">
                        <span>
                          入 {fmtRate(cand.entry.input)} · 出 {fmtRate(cand.entry.output)}
                        </span>
                        {selected ? (
                          <svg
                            viewBox="0 0 24 24"
                            width={13}
                            height={13}
                            fill="none"
                            stroke="currentColor"
                            strokeWidth={2.4}
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            aria-hidden="true"
                            className="shrink-0 text-primary"
                          >
                            <path d="M20 6 9 17l-5-5" />
                          </svg>
                        ) : (
                          <span className="h-3.5 w-3.5 shrink-0" />
                        )}
                      </span>
                    </button>
                  );
                })}
              </div>
            </div>,
            document.body,
          )
        : null}
    </>
  );
}
