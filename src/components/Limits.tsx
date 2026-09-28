import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import {
  api,
  LIMIT_PROVIDER_LABELS,
  type Balance,
  type ProviderLimits,
  type QuotaWindow,
} from "../api/bindings";
import { fmtClock, fmtDate, fmtDateTime } from "../lib/format";
import {
  applyLayout,
  hitTestBands,
  parseRowKey,
  parseStoredLayout,
  reconcileLayout,
  rowKey,
  sameLayout,
  type LayoutDrop,
  type RowBand,
} from "../lib/limitsLayout";
import { GripVerticalIcon, RefreshIcon } from "./icons";
import { EmptyState, Skeleton } from "./Skeleton";

/** 这台机器上的行序,不是账号配置,所以不进 settings.json */
const LAYOUT_KEY = "token-show-limits-layout";

const POLL_INTERVAL_MS = 60_000;

/** 剩余百分比的配色:绿 ≥50、橙 ≥25、红 <25 */
function remainingTone(usedPercent: number): string {
  const left = 100 - usedPercent;
  if (left >= 50) return "bg-emerald-500";
  if (left >= 25) return "bg-amber-500";
  return "bg-red-500";
}

function remainingText(usedPercent: number): string {
  const left = 100 - usedPercent;
  if (left >= 50) return "text-emerald-500";
  if (left >= 25) return "text-amber-500";
  return "text-red-500";
}

/** 重置倒计时:只说到"天/小时/分钟"这一档,秒级跳动没有信息量 */
function countdown(
  resetAt: number | null | undefined,
  now: number,
): string | null {
  if (!resetAt) return null;
  const ms = resetAt - now;
  if (ms <= 0) return "即将重置";
  const mins = Math.floor(ms / 60_000);
  if (mins < 60) return `${mins} 分钟后重置`;
  const hours = Math.floor(mins / 60);
  if (hours < 48) return `${hours} 小时后重置`;
  return `${Math.floor(hours / 24)} 天后重置`;
}

/** 到期类窗口的剩余时间。到期不是"重置",不能复用 countdown 的措辞 */
function expiryLeft(expireAt: number, now: number): string {
  const ms = expireAt - now;
  if (ms <= 0) return "已到期";
  const days = Math.floor(ms / 86_400_000);
  return days < 1 ? "不足 1 天" : `剩 ${days} 天`;
}

/** 到期类窗口(如"套餐有效期"):只有日期,没有用量百分比 */
function isDateOnly(w: QuotaWindow): boolean {
  return typeof w.usedPercent !== "number" && !!w.resetAt;
}

/**
 * 一个用量窗口:名称 + 剩余百分比 / 进度条 / 重置倒计时。
 *
 * 所有格子(含余额格)都是同样的三段,空的那段也占位:同一行里的进度条
 * 和脚注才对得齐,只有余额的行也不会比有三个窗口的行矮一截。
 */
function QuotaCell({ w, now }: { w: QuotaWindow; now: number }) {
  // usedPercent 为 null 表示上游没给这个字段。
  // 这时**不渲染进度条**:空进度条会让人以为额度见底(或全满),是反向误导。
  const used = typeof w.usedPercent === "number" ? w.usedPercent : null;
  const left = used == null ? null : Math.max(0, 100 - used);
  const cd = countdown(w.resetAt, now);
  const tip = [
    used == null ? "上游未返回用量" : `已用 ${used.toFixed(1)}%`,
    w.resetAt ? `重置于 ${fmtDateTime(w.resetAt)}` : null,
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <div className="min-w-0 space-y-1.5" title={tip}>
      <div className="flex h-5 items-baseline justify-between gap-2">
        <span className="truncate text-xs text-muted-foreground">{w.label}</span>
        {left == null || used == null ? (
          <span className="font-mono text-sm text-muted-foreground">--</span>
        ) : (
          <span className="flex shrink-0 items-baseline gap-1">
            <span
              className={`font-mono text-sm font-semibold tabular-nums ${remainingText(used)}`}
            >
              {left.toFixed(left < 10 ? 1 : 0)}%
            </span>
            <span className="text-[10px] text-muted-foreground">剩余</span>
          </span>
        )}
      </div>
      {left == null || used == null ? (
        <div className="h-1.5" />
      ) : (
        <div className="h-1.5 overflow-hidden rounded-full bg-muted">
          <div
            className={`h-full rounded-full transition-all ${remainingTone(used)}`}
            style={{ width: `${Math.min(100, left)}%` }}
          />
        </div>
      )}
      <p className="h-4 text-[10px] leading-4 text-muted-foreground">{cd}</p>
    </div>
  );
}

function BalanceCell({ b }: { b: Balance }) {
  const parts = [
    b.cash != null ? `充值 ${b.cash.toFixed(2)}` : null,
    b.voucher != null ? `赠送 ${b.voucher.toFixed(2)}` : null,
  ].filter(Boolean);
  return (
    <div className="min-w-0 space-y-1.5">
      <div className="flex h-5 items-baseline justify-between gap-2">
        <span className="text-xs text-muted-foreground">余额</span>
        <span className="flex shrink-0 items-baseline gap-1">
          <span className="font-mono text-sm font-semibold tabular-nums">
            {b.amount.toFixed(2)}
          </span>
          <span className="text-[10px] text-muted-foreground">{b.currency}</span>
        </span>
      </div>
      <div className="h-1.5" />
      <p className="h-4 text-[10px] leading-4 text-muted-foreground">
        {parts.join(" · ")}
      </p>
    </div>
  );
}

/**
 * 子窗口的排列规则:短周期用量在前,长周期在后,到期类垫底。
 *
 * 后端给的顺序不能直接用 —— 各 provider 的拼装方式不同:
 * Cursor 把套餐额度 insert 到 0 位再把分道百分比追加到末尾,
 * StepFun 把「套餐有效期」push 到最后。照原样渲染时同一张卡片里
 * 「5 小时」可能排在「每月」后面,两张卡片的行序也对不齐。
 */
const WINDOW_ORDER = [
  "five_hour",
  "weekly",
  "monthly",
  "plan",
  "auto",
  "api",
  "credit",
  "topup",
  "balance",
];

/** 排序档位:0 = 有窗口长度的用量窗口,1 = 没有长度的用量窗口,2 = 到期类 */
function windowRank(w: QuotaWindow): [number, number] {
  // 到期类(只有日期、没有用量)不是「用了多少」的窗口,统一垫底
  if (isDateOnly(w)) return [2, w.resetAt ?? 0];
  // 有窗口长度就按长度升序:5 小时 → 每周 → 每月
  if (typeof w.windowSeconds === "number" && w.windowSeconds > 0) {
    return [0, w.windowSeconds];
  }
  // 没有长度时按语义排;认不出的 key 排在所有已知 key 之后
  const i = WINDOW_ORDER.indexOf(w.key);
  return [1, i < 0 ? WINDOW_ORDER.length : i];
}

/** 稳定排序:同档位内保持后端给的先后顺序,不凭空调换 */
function sortWindows(windows: QuotaWindow[]): QuotaWindow[] {
  return windows
    .map((w, i) => ({ w, i, rank: windowRank(w) }))
    .sort((a, b) =>
      a.rank[0] - b.rank[0] || a.rank[1] - b.rank[1] || a.i - b.i,
    )
    .map((x) => x.w);
}

const PROVIDER_ORDER = ["cursor", "codex", "deepseek", "qwen", "stepfun"];

/**
 * 组内顺序:已配置在前,内置账号(id 与 provider 同名)在前,再按账号名。
 * 后端返回顺序不可靠;不先排内置,「备用」会按拼音排到「Codex CLI」前面。
 */
function sortCards(list: ProviderLimits[]): ProviderLimits[] {
  return [...list].sort((a, b) => {
    if (a.configured !== b.configured) return a.configured ? -1 : 1;
    const builtinA = a.accountId === a.provider;
    const builtinB = b.accountId === b.provider;
    if (builtinA !== builtinB) return builtinA ? -1 : 1;
    return (a.accountLabel || a.accountId).localeCompare(
      b.accountLabel || b.accountId,
      "zh-Hans-CN",
    );
  });
}

/** 行的排布顺序:先按 provider 固定档位,组内再按 sortCards */
function orderedCards(items: ProviderLimits[]): ProviderLimits[] {
  const buckets = new Map<string, ProviderLimits[]>();
  for (const it of items) {
    const list = buckets.get(it.provider) ?? [];
    list.push(it);
    buckets.set(it.provider, list);
  }
  const keys = [
    ...PROVIDER_ORDER.filter((p) => buckets.has(p)),
    ...[...buckets.keys()].filter((p) => !PROVIDER_ORDER.includes(p)),
  ];
  return keys.flatMap((p) => sortCards(buckets.get(p) ?? []));
}

/**
 * 格子列数。到期日写在账号名下面,不占格子;余额占一列。
 * 只有一列时才允许跟另一条单列行并成一行,多列行要留出整行才能横向比较。
 */
function columnCount(data: ProviderLimits): number {
  let n = 0;
  for (const w of data.windows) if (!isDateOnly(w)) n += 1;
  if (data.balance != null) n += 1;
  return n;
}

function isSingleColumn(data: ProviderLimits): boolean {
  return columnCount(data) === 1;
}

function readLayout(): string[][] | null {
  try {
    return parseStoredLayout(localStorage.getItem(LAYOUT_KEY));
  } catch {
    return null;
  }
}

/** 没拖过用默认顺序;拖过的按存储套上来,并丢掉已经不存在的账号 */
function resolveRows(items: ProviderLimits[], saved: string[][] | null): string[][] {
  const order: string[] = [];
  const seen = new Set<string>();
  for (const it of orderedCards(items)) {
    if (seen.has(it.accountId)) continue;
    seen.add(it.accountId);
    order.push(it.accountId);
  }
  if (!saved) return order.map((id) => [id]);
  const single = new Set<string>();
  for (const it of items) {
    if (isSingleColumn(it)) single.add(it.accountId);
  }
  return reconcileLayout(order, saved, single);
}

/**
 * 一个账号的左名右格。full 是独占一行的三列网格;half 是合并行里的一半,
 * 只有一列,不再套三列网格,否则半行里会空出两大块。
 */
function AccountFace({
  data,
  now,
  variant,
}: {
  data: ProviderLimits;
  now: number;
  variant: "full" | "half";
}) {
  const providerLabel = LIMIT_PROVIDER_LABELS[data.provider] ?? data.provider;
  const name = data.accountLabel || providerLabel;
  const quota = sortWindows(data.windows.filter((w) => !isDateOnly(w)));
  const expiries = data.windows.filter(isDateOnly);
  const hasData = data.windows.length > 0 || data.balance != null;

  // 内置账号的名字就是 provider 名,再标一次就成了「Cursor / Cursor」
  const meta: ReactNode[] = [];
  if (!name.toLowerCase().includes(providerLabel.toLowerCase())) {
    meta.push(providerLabel);
  }
  for (const w of expiries) {
    meta.push(
      <span title={w.label}>
        {fmtDate(w.resetAt)} 到期 · {expiryLeft(w.resetAt as number, now)}
      </span>,
    );
  }

  const cells = (
    <>
      {quota.map((w) => (
        <QuotaCell key={w.key} w={w} now={now} />
      ))}
      {data.balance ? <BalanceCell b={data.balance} /> : null}
    </>
  );

  return (
    <div
      className={`flex min-w-0 flex-1 flex-col gap-3 md:flex-row md:gap-6 ${
        hasData ? "" : "md:items-center"
      }`}
    >
      <div className="min-w-0 md:w-48 md:shrink-0">
        <h3 className="flex items-center gap-1.5 text-sm font-semibold">
          <span className="truncate">{name}</span>
          {data.planLabel ? (
            <span className="shrink-0 rounded-md bg-primary/10 px-1.5 py-0.5 text-[10px] font-medium text-primary">
              {data.planLabel}
            </span>
          ) : null}
          {!data.configured ? (
            <span className="shrink-0 rounded-md bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-medium text-amber-600 dark:text-amber-400">
              待配置
            </span>
          ) : null}
        </h3>
        {meta.length > 0 ? (
          <p className="mt-1 flex flex-wrap items-center gap-x-1.5 text-[11px] text-muted-foreground">
            {meta.map((m, i) => (
              <span key={i} className="flex items-center gap-x-1.5">
                {i > 0 ? <span aria-hidden>·</span> : null}
                {m}
              </span>
            ))}
          </p>
        ) : null}
        {hasData && data.error ? (
          <p
            className="mt-1 text-[11px] text-amber-600 dark:text-amber-400"
            title={data.error}
          >
            刷新失败,显示 {fmtClock(data.fetchedAt)} 的缓存
          </p>
        ) : null}
      </div>

      {hasData ? (
        variant === "full" ? (
          <div className="grid min-w-0 flex-1 grid-cols-3 gap-x-6 gap-y-3">{cells}</div>
        ) : (
          // 单列格子在整行里只占三列网格的第一列。合并后如果跟着半行拉宽,
          // 进度条会变成原来的两倍多。宽度按「整行去掉手柄和账号名,再三等分」。
          <div className="grid w-full min-w-0 gap-y-3 md:w-[calc((100cqi-18.25rem)/3)] md:max-w-full md:flex-none">
            {cells}
          </div>
        )
      ) : (
        <p
          className={`min-w-0 flex-1 text-xs leading-relaxed ${
            data.configured ? "text-red-500" : "text-muted-foreground"
          }`}
        >
          {data.error ?? "暂无数据"}
        </p>
      )}
    </div>
  );
}

function AccountHalf({
  data,
  now,
  dimmed,
  onGripDown,
}: {
  data: ProviderLimits;
  now: number;
  dimmed: boolean;
  onGripDown: (event: ReactPointerEvent<HTMLButtonElement>) => void;
}) {
  // gap-2 与整行的手柄间距相同,进度条左缘才和整行第一列对齐。
  return (
    <div className={`flex min-w-0 flex-1 items-start gap-2 ${dimmed ? "opacity-40" : ""}`}>
      <DragGrip
        label="拖出此账号"
        className="mt-0.5 md:mt-0 md:self-center"
        onPointerDown={onGripDown}
      />
      <AccountFace data={data} now={now} variant="half" />
    </div>
  );
}

function DragGrip({
  label,
  className,
  onPointerDown,
}: {
  label: string;
  className?: string;
  onPointerDown: (event: ReactPointerEvent<HTMLButtonElement>) => void;
}) {
  return (
    <button
      type="button"
      draggable={false}
      aria-label={label}
      title={label}
      onPointerDown={onPointerDown}
      className={`flex h-8 w-5 shrink-0 cursor-grab touch-none items-center justify-center rounded-md text-muted-foreground/45 hover:bg-black/5 hover:text-foreground active:cursor-grabbing group-hover/row:text-muted-foreground/80 dark:hover:bg-white/5 ${
        className ?? ""
      }`}
    >
      <GripVerticalIcon className="h-4 w-4" />
    </button>
  );
}

/** 合并行中间的分隔线。平时只是一条线,悬停才露出手柄,避免左边叠两个拖动条。 */
function SplitHandle({
  onPointerDown,
}: {
  onPointerDown: (event: ReactPointerEvent<HTMLButtonElement>) => void;
}) {
  return (
    <button
      type="button"
      draggable={false}
      aria-label="拖动整行"
      title="拖动整行"
      onPointerDown={onPointerDown}
      className="group/split relative flex h-6 w-full shrink-0 cursor-grab touch-none items-center justify-center self-center md:mx-2 md:h-8 md:w-5"
    >
      <span
        aria-hidden
        className="absolute left-0 right-0 top-1/2 h-px -translate-y-1/2 bg-border md:bottom-0 md:left-1/2 md:right-auto md:top-0 md:h-auto md:w-px md:-translate-x-1/2 md:translate-y-0"
      />
      <GripVerticalIcon className="relative z-10 h-4 w-4 text-transparent group-hover/split:text-muted-foreground/80" />
    </button>
  );
}

interface DragSession {
  ids: string[];
  rowKey: string;
  pointerId: number;
  startX: number;
  startY: number;
  /** 最近一次指针的视口 Y,松手时用它算落点 */
  endY: number;
  active: boolean;
  move: (event: PointerEvent) => void;
  up: (event: PointerEvent) => void;
  key: (event: KeyboardEvent) => void;
}

function stopSession(session: DragSession) {
  window.removeEventListener("pointermove", session.move);
  window.removeEventListener("pointerup", session.up);
  window.removeEventListener("pointercancel", session.up);
  window.removeEventListener("keydown", session.key);
}

export function Limits({ refreshEpoch }: { refreshEpoch: number }) {
  const [items, setItems] = useState<ProviderLimits[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  /** null = 用户还没拖过,走 provider 默认顺序 */
  const [layout, setLayout] = useState<string[][] | null>(readLayout);
  const [drag, setDrag] = useState<{ ids: string[]; rowKey: string } | null>(null);
  const [drop, setDrop] = useState<LayoutDrop | null>(null);
  const inFlightRef = useRef(false);
  const pendingRef = useRef<"none" | "cache" | "force">("none");
  const seenEpoch = useRef(refreshEpoch);
  const loadRef = useRef<(force: boolean) => Promise<void>>(async () => {});
  const listRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<DragSession | null>(null);
  const rowsRef = useRef<string[][]>([]);
  const itemsRef = useRef<ProviderLimits[] | null>(null);
  const singleRef = useRef<Set<string>>(new Set());

  const load = useCallback(async (force: boolean) => {
    if (inFlightRef.current) {
      pendingRef.current =
        force || pendingRef.current === "force" ? "force" : "cache";
      return;
    }
    inFlightRef.current = true;
    setLoading(true);
    try {
      // 先读缓存秒开,再按需真的拉一次
      const cached = await api.getLimits();
      setItems(cached);
      if (force || cached.length === 0) {
        const fresh = await api.refreshLimits();
        setItems(fresh);
      }
      setError(null);
    } catch (err) {
      console.error("[Limits] 拉取失败", err);
      setError(String(err));
    } finally {
      setLoading(false);
      inFlightRef.current = false;
      const pending = pendingRef.current;
      pendingRef.current = "none";
      if (pending !== "none") void loadRef.current(pending === "force");
    }
  }, []);
  loadRef.current = load;

  useEffect(() => {
    void load(false);
  }, [load]);

  // 顶栏刷新已经把额度写进缓存,这里只重读,不再发一遍网络
  useEffect(() => {
    if (seenEpoch.current === refreshEpoch) return;
    seenEpoch.current = refreshEpoch;
    void load(false);
  }, [refreshEpoch, load]);

  // 后端 60 秒刷一轮,前端跟着走;倒计时用本地时钟每秒重算
  useEffect(() => {
    const poll = window.setInterval(() => void load(false), POLL_INTERVAL_MS);
    const tick = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => {
      window.clearInterval(poll);
      window.clearInterval(tick);
    };
  }, [load]);

  // 各行刷新时刻基本相同,统一放页头;刷新失败的行在自己那一行标出缓存时刻
  const lastFetched = items?.reduce((m, it) => Math.max(m, it.fetchedAt), 0) ?? 0;

  const rows = useMemo(
    () => (items ? resolveRows(items, layout) : null),
    [items, layout],
  );
  const byId = useMemo(() => {
    const map = new Map<string, ProviderLimits>();
    for (const it of items ?? []) map.set(it.accountId, it);
    return map;
  }, [items]);
  const singleIds = useMemo(() => {
    const single = new Set<string>();
    for (const it of items ?? []) {
      if (isSingleColumn(it)) single.add(it.accountId);
    }
    return single;
  }, [items]);

  itemsRef.current = items;
  rowsRef.current = rows ?? [];
  singleRef.current = singleIds;

  useEffect(() => {
    return () => {
      const session = dragRef.current;
      if (session) stopSession(session);
      dragRef.current = null;
    };
  }, []);

  useEffect(() => {
    if (!drag) return;
    const body = document.body;
    const prevCursor = body.style.cursor;
    const prevSelect = body.style.userSelect;
    const prevTouch = body.style.touchAction;
    body.style.cursor = "grabbing";
    body.style.userSelect = "none";
    body.style.touchAction = "none";
    return () => {
      body.style.cursor = prevCursor;
      body.style.userSelect = prevSelect;
      body.style.touchAction = prevTouch;
    };
  }, [drag]);

  const locate = (y: number, movingIds: readonly string[]): LayoutDrop | null => {
    const root = listRef.current;
    if (!root) return null;
    const bands: RowBand[] = [];
    for (const el of root.querySelectorAll<HTMLElement>("[data-limit-row]")) {
      const rect = el.getBoundingClientRect();
      const ids = parseRowKey(el.dataset.limitRow ?? "");
      if (ids.length === 0) continue;
      bands.push({ top: rect.top, bottom: rect.bottom, ids });
    }
    return hitTestBands(bands, y, movingIds, singleRef.current);
  };

  const persist = (next: string[][]) => {
    const itemsNow = itemsRef.current;
    const resolved = itemsNow ? resolveRows(itemsNow, next) : next;
    if (sameLayout(rowsRef.current, resolved)) return;
    setLayout(resolved);
    try {
      localStorage.setItem(LAYOUT_KEY, JSON.stringify(resolved));
    } catch (err) {
      console.error("[Limits] 无法保存行顺序", err);
    }
  };

  const begin = (
    event: ReactPointerEvent<HTMLButtonElement>,
    payload: { ids: string[]; rowKey: string },
  ) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const previous = dragRef.current;
    if (previous) stopSession(previous);

    const session: DragSession = {
      ids: payload.ids.slice(),
      rowKey: payload.rowKey,
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      endY: event.clientY,
      active: false,
      move: () => {},
      up: () => {},
      key: () => {},
    };

    const finish = (commit: boolean) => {
      stopSession(session);
      if (dragRef.current !== session) return;
      dragRef.current = null;
      if (commit && session.active) {
        const target = locate(session.endY, session.ids);
        if (target) persist(applyLayout(rowsRef.current, session.ids, target));
      }
      setDrag(null);
      setDrop(null);
    };

    session.move = (ev: PointerEvent) => {
      if (ev.pointerId !== session.pointerId) return;
      session.endY = ev.clientY;
      if (!session.active) {
        if (Math.hypot(ev.clientX - session.startX, ev.clientY - session.startY) < 4) {
          return;
        }
        session.active = true;
        setDrag({ ids: session.ids, rowKey: session.rowKey });
      }
      ev.preventDefault();
      const target = locate(ev.clientY, session.ids);
      const next =
        target && !sameLayout(rowsRef.current, applyLayout(rowsRef.current, session.ids, target))
          ? target
          : null;
      setDrop((prev) => {
        if (next === null) return prev === null ? prev : null;
        if (
          prev &&
          prev.mode === next.mode &&
          rowKey(prev.rowIds) === rowKey(next.rowIds)
        ) {
          return prev;
        }
        return next;
      });
    };
    session.up = (ev: PointerEvent) => {
      if (ev.pointerId !== session.pointerId) return;
      session.endY = ev.clientY;
      finish(ev.type !== "pointercancel");
    };
    session.key = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      finish(false);
    };

    dragRef.current = session;
    window.addEventListener("pointermove", session.move);
    window.addEventListener("pointerup", session.up);
    window.addEventListener("pointercancel", session.up);
    window.addEventListener("keydown", session.key);
  };

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-lg font-semibold">额度</h2>
        <div className="flex items-center gap-3">
          {lastFetched > 0 ? (
            <span
              className="text-xs text-muted-foreground"
              title={fmtDateTime(lastFetched)}
            >
              更新于 {fmtClock(lastFetched)}
            </span>
          ) : null}
          <button
            type="button"
            disabled={loading}
            onClick={() => void load(true)}
            className="flex h-8 items-center gap-1.5 rounded-lg border border-border px-3 text-xs font-medium transition-colors hover:bg-black/5 disabled:opacity-50 dark:hover:bg-white/5"
          >
            <RefreshIcon className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
            刷新
          </button>
        </div>
      </div>

      {items == null ? (
        <div className="space-y-2">
          <Skeleton className="h-20" />
          <Skeleton className="h-20" />
          <Skeleton className="h-20" />
        </div>
      ) : items.length === 0 ? (
        <EmptyState message="还没有启用任何额度来源。到设置 → 额度来源里勾选需要显示的 Provider。" />
      ) : (
        <div ref={listRef} className={`space-y-2 ${drag ? "select-none" : ""}`}>
          {rows?.map((ids) => {
            const key = rowKey(ids);
            const records: ProviderLimits[] = [];
            for (const id of ids) {
              const data = byId.get(id);
              if (data) records.push(data);
            }
            if (records.length !== ids.length) return null;
            const paired = records.length === 2;
            const wholeDragged =
              drag != null &&
              drag.rowKey === key &&
              drag.ids.length === ids.length &&
              drag.ids.every((id, index) => id === ids[index]);
            const gripLabel = singleIds.has(ids[0])
              ? "拖动排序，拖到另一条单列行中间可合并"
              : "拖动排序";
            const merging = drop?.mode === "merge" && rowKey(drop.rowIds) === key;
            const insertBefore = drop?.mode === "before" && rowKey(drop.rowIds) === key;
            const insertAfter = drop?.mode === "after" && rowKey(drop.rowIds) === key;
            const [left, right] = records;
            return (
              <div key={key} className="relative" data-limit-row={key}>
                {insertBefore ? (
                  <div className="pointer-events-none absolute -top-1 left-4 right-4 z-10 h-0.5 rounded-full bg-primary" />
                ) : null}
                {insertAfter ? (
                  <div className="pointer-events-none absolute -bottom-1 left-4 right-4 z-10 h-0.5 rounded-full bg-primary" />
                ) : null}
                <section
                  className={`group/row relative flex items-stretch gap-2 rounded-xl border bg-card px-4 py-3.5 [container-type:inline-size] transition-[border-color,box-shadow,background-color] duration-300 md:min-h-[84px] ${
                    merging
                      ? "border-primary bg-primary/5 shadow-sm"
                      : "border-border hover:border-primary/60 hover:shadow-sm"
                  } ${wholeDragged ? "border-dashed opacity-40" : ""}`}
                >
                  {paired && left && right ? (
                    <div className="flex min-w-0 flex-1 flex-col gap-3 md:flex-row md:items-start">
                      <AccountHalf
                        data={left}
                        now={now}
                        dimmed={
                          drag != null &&
                          drag.rowKey === key &&
                          drag.ids.length === 1 &&
                          drag.ids[0] === left.accountId
                        }
                        onGripDown={(event) =>
                          begin(event, { ids: [left.accountId], rowKey: key })
                        }
                      />
                      <SplitHandle
                        onPointerDown={(event) => begin(event, { ids, rowKey: key })}
                      />
                      <AccountHalf
                        data={right}
                        now={now}
                        dimmed={
                          drag != null &&
                          drag.rowKey === key &&
                          drag.ids.length === 1 &&
                          drag.ids[0] === right.accountId
                        }
                        onGripDown={(event) =>
                          begin(event, { ids: [right.accountId], rowKey: key })
                        }
                      />
                    </div>
                  ) : (
                    <>
                      <DragGrip
                        label={gripLabel}
                        className="mt-0.5 self-start md:mt-0 md:self-center"
                        onPointerDown={(event) => begin(event, { ids, rowKey: key })}
                      />
                      <AccountFace data={records[0]} now={now} variant="full" />
                    </>
                  )}
                  {merging ? (
                    <span className="pointer-events-none absolute right-3 top-3.5 z-10 rounded-md bg-primary px-1.5 py-0.5 text-[10px] font-medium text-primary-foreground">
                      松开合并
                    </span>
                  ) : null}
                </section>
              </div>
            );
          })}
        </div>
      )}

      {error ? <p className="text-xs text-red-500">{error}</p> : null}
    </div>
  );
}
