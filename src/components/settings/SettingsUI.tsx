import { useState, type ReactNode } from "react";
import { ChevronDownIcon } from "../icons";

export function Badge({
  tone,
  children,
}: {
  tone: "good" | "muted";
  children: ReactNode;
}) {
  return (
    <span
      data-theme-part="badge"
      className={`rounded-md px-1.5 py-0.5 text-xs ${
        tone === "good"
          ? "bg-success/10 text-success-text"
          : "bg-muted text-muted-foreground"
      }`}
    >
      {children}
    </span>
  );
}

/**
 * 自制开关:w-11 h-6 轨道,开 = bg-success、关 = mutedForeground 30%;
 * 滑块颜色是主题的 switchThumb / switchThumbOff,平移动画。设置页所有开关都用它。
 */
export function Toggle({
  checked,
  disabled,
  onChange,
  ariaLabel,
}: {
  checked: boolean;
  disabled?: boolean;
  onChange: () => void;
  ariaLabel: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={onChange}
      data-theme-part="switch"
      data-theme-state={checked ? "selected" : undefined}
      className={`relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 ${
        checked ? "bg-success" : "bg-muted-foreground/30"
      } disabled:cursor-not-allowed disabled:opacity-50`}
    >
      <span
        data-theme-part="switch-thumb"
        className={`inline-block h-5 w-5 rounded-full shadow transition-[transform,background-color] duration-200 ${
          checked ? "translate-x-5 bg-switch-thumb" : "translate-x-0.5 bg-switch-thumb-off"
        }`}
      />
    </button>
  );
}

export function SectionCard({
  icon,
  title,
  children,
  collapsible = false,
  summary,
}: {
  icon: ReactNode;
  title: string;
  children: ReactNode;
  /** 可折叠。默认收起,展开与否记在本机,下次打开设置还是这个状态 */
  collapsible?: boolean;
  /** 收起时的一行摘要,比如「3 个来源已开启」 */
  summary?: string;
}) {
  const [open, setOpen] = useState(() => {
    if (!collapsible) return true;
    try {
      return localStorage.getItem(`token-show-fold:${title}`) === "1";
    } catch {
      return false;
    }
  });

  const toggle = () => {
    setOpen((v) => {
      const next = !v;
      try {
        localStorage.setItem(`token-show-fold:${title}`, next ? "1" : "0");
      } catch {
        // 写不进去就只在这次打开里生效
      }
      return next;
    });
  };

  const head = (
    <>
      <h3 data-theme-part="card-title" className="flex items-center gap-1.5 text-sm font-semibold">
        {icon}
        {title}
      </h3>
      {collapsible && !open && summary ? (
        <p className="mt-0.5 text-xs text-muted-foreground">{summary}</p>
      ) : null}
    </>
  );

  return (
    <section
      data-theme-part="card settings-section"
      className="overflow-hidden rounded-xl border border-border bg-card transition-all duration-300 hover:border-primary/60 hover:shadow-sm"
    >
      {collapsible ? (
        <button
          type="button"
          onClick={toggle}
          aria-expanded={open}
          className="flex w-full items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-overlay/5"
        >
          <div className="min-w-0 flex-1">{head}</div>
          <ChevronDownIcon
            className={`h-4 w-4 shrink-0 text-muted-foreground transition-transform ${
              open ? "" : "-rotate-90"
            }`}
          />
        </button>
      ) : (
        <div className="border-b border-border/40 px-4 py-3">{head}</div>
      )}
      {/* 收起时藏起来但不卸载:定价表和账号表单的未保存输入还在 */}
      <div
        className={
          collapsible ? (open ? "border-t border-border/40 animate-fade-soft" : "hidden") : undefined
        }
      >
        {children}
      </div>
    </section>
  );
}
