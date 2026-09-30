import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { createPortal } from "react-dom";
import { ChevronDownIcon } from "./icons";

export type SelectOption = {
  value: string;
  label: string;
  disabled?: boolean;
};

type MenuPos = {
  left: number;
  width: number;
  maxHeight: number;
  top?: number;
  bottom?: number;
};

/**
 * 自绘下拉。原生 select 的弹出层是系统菜单,跟不上主题。
 * 菜单挂到 body 上,避免被设置分区卡的 overflow:hidden 裁掉。
 */
export function SelectMenu({
  value,
  onChange,
  options,
  disabled,
  title,
  className,
  ariaLabel,
}: {
  value: string;
  onChange: (value: string) => void;
  options: readonly SelectOption[];
  disabled?: boolean;
  title?: string;
  /** 触发器尺寸,例如 h-7、w-full、max-w-[220px] */
  className?: string;
  ariaLabel?: string;
}) {
  const listId = useId();
  const btnRef = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [pos, setPos] = useState<MenuPos | null>(null);

  const current = options.find((o) => o.value === value);
  const label = current?.label ?? value;

  const measure = () => {
    const el = btnRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const margin = 8;
    const gap = 6;
    const spaceBelow = window.innerHeight - r.bottom - gap - margin;
    const spaceAbove = r.top - gap - margin;
    const flip = spaceBelow < 160 && spaceAbove > spaceBelow;
    const maxHeight = Math.max(96, Math.min(280, flip ? spaceAbove : spaceBelow));
    const width = Math.min(
      Math.max(r.width, 168),
      window.innerWidth - margin * 2,
    );
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
      if (node instanceof Element && node.getAttribute("role") === "listbox") return;
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
  }, [open, options]);

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
        return;
      }
      if (e.key === "ArrowDown") {
        e.preventDefault();
        step(1);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        step(-1);
        return;
      }
      if (e.key === "Home") {
        e.preventDefault();
        setActive(firstEnabled(options));
        return;
      }
      if (e.key === "End") {
        e.preventDefault();
        setActive(lastEnabled(options));
        return;
      }
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        commit(active);
        return;
      }
      if (e.key === "Tab") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, active, options, listId]);

  useEffect(() => {
    if (!open || active < 0) return;
    document.getElementById(`${listId}-opt-${active}`)?.scrollIntoView({ block: "nearest" });
  }, [open, active, listId]);

  const openAt = (prefer: "selected" | "next" | "prev") => {
    if (disabled) return;
    const selected = options.findIndex((o) => o.value === value && !o.disabled);
    let idx = selected >= 0 ? selected : firstEnabled(options);
    if (prefer === "next") idx = neighbor(options, idx, 1);
    if (prefer === "prev") idx = neighbor(options, idx < 0 ? 0 : idx, -1);
    setActive(idx);
    setOpen(true);
  };

  const step = (dir: 1 | -1) => {
    setActive((cur) => neighbor(options, cur, dir));
  };

  const commit = (index: number) => {
    const opt = options[index];
    if (!opt || opt.disabled) return;
    onChange(opt.value);
    setOpen(false);
    btnRef.current?.focus();
  };

  const onTriggerKey = (e: ReactKeyboardEvent<HTMLButtonElement>) => {
    if (disabled || open) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      openAt(e.key === "ArrowUp" ? "prev" : e.key === "ArrowDown" ? "next" : "selected");
    }
  };

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        disabled={disabled}
        title={title}
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        data-theme-part="input"
        onClick={() => (open ? setOpen(false) : openAt("selected"))}
        onKeyDown={onTriggerKey}
        className={`inline-flex min-w-0 items-center justify-between gap-1.5 rounded-lg border bg-background px-2.5 text-left text-xs text-foreground outline-none transition-colors focus-visible:border-primary disabled:cursor-not-allowed disabled:opacity-50 ${
          open ? "border-primary" : "border-border hover:border-primary/50"
        } ${className ?? "h-8"}`}
      >
        <span className="min-w-0 flex-1 truncate">{label}</span>
        <ChevronDownIcon
          className={`h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform duration-150 ${
            open ? "rotate-180" : ""
          }`}
        />
      </button>
      {open && pos
        ? createPortal(
            <div
              id={listId}
              role="listbox"
              aria-label={ariaLabel}
              style={{
                position: "fixed",
                left: pos.left,
                width: pos.width,
                maxHeight: pos.maxHeight,
                top: pos.top,
                bottom: pos.bottom,
                zIndex: 70,
              }}
              className="overflow-auto rounded-xl border border-border bg-popover p-1 text-popover-foreground shadow-lg"
            >
              {options.map((opt, i) => {
                const selected = opt.value === value;
                const isActive = i === active;
                return (
                  <div
                    key={opt.value}
                    id={`${listId}-opt-${i}`}
                    role="option"
                    aria-selected={selected}
                    aria-disabled={opt.disabled || undefined}
                    title={opt.label}
                    onMouseEnter={() => {
                      if (!opt.disabled) setActive(i);
                    }}
                    onMouseDown={(e) => {
                      e.preventDefault();
                      commit(i);
                    }}
                    className={`flex items-center gap-2 rounded-lg px-2 py-1.5 text-xs ${itemClass(
                      selected,
                      isActive,
                      !!opt.disabled,
                    )} ${selected ? "font-medium" : ""}`}
                  >
                    <span className="min-w-0 flex-1 truncate">{opt.label}</span>
                    {selected ? (
                      <svg
                        viewBox="0 0 24 24"
                        width={14}
                        height={14}
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
                  </div>
                );
              })}
            </div>,
            document.body,
          )
        : null}
    </>
  );
}

function itemClass(selected: boolean, active: boolean, disabled: boolean): string {
  if (disabled) return "cursor-not-allowed text-muted-foreground/50";
  if (selected && active) return "cursor-pointer bg-primary/15 text-foreground";
  if (selected) return "cursor-pointer bg-primary/10 text-foreground";
  if (active) return "cursor-pointer bg-accent text-accent-foreground";
  return "cursor-pointer text-popover-foreground";
}

function firstEnabled(options: readonly SelectOption[]): number {
  return options.findIndex((o) => !o.disabled);
}

function lastEnabled(options: readonly SelectOption[]): number {
  for (let i = options.length - 1; i >= 0; i--) {
    if (!options[i]?.disabled) return i;
  }
  return -1;
}

function neighbor(options: readonly SelectOption[], from: number, dir: 1 | -1): number {
  if (options.length === 0) return -1;
  let i = from;
  for (let n = 0; n < options.length; n++) {
    i = (i + dir + options.length) % options.length;
    if (!options[i]?.disabled) return i;
  }
  return from;
}
