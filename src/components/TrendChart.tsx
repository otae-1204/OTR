import { useEffect, useMemo, useState } from "react";
import dayjs from "dayjs";
import {
  Area,
  AreaChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import {
  agentColor,
  AGENT_LABELS,
  api,
  type DailyUsage,
} from "../api/bindings";
import { fmtTokens } from "../lib/format";
import { EmptyState, Skeleton } from "./Skeleton";
import { TrendingUpIcon } from "./icons";

const KNOWN_ORDER = ["dsh", "claude-code", "codex", "zcode", "opencode", "pi"];
const MAX_DAYS = 366;

type PivotRow = Record<string, string | number>;

function agentLabel(id: string): string {
  return AGENT_LABELS[id] ?? id;
}

function TrendTooltip({
  active,
  payload,
  label,
  granularity,
  crossYear,
}: any) {
  if (!active || !payload || payload.length === 0) return null;
  const total = payload.reduce(
    (acc: number, p: any) => acc + (Number(p.value) || 0),
    0,
  );
  const labelText =
    granularity === "hour" || granularity === "month" || crossYear
      ? String(label)
      : String(label).slice(5);
  return (
    <div
      className="px-3 py-2 shadow-lg"
      style={{
        background: "hsl(var(--card))",
        border: "1px solid hsl(var(--border))",
        borderRadius: 12,
        fontSize: 12,
      }}
    >
      <div className="mb-1 flex items-center justify-between gap-4">
        <span className="font-medium text-foreground">{labelText}</span>
        <span className="font-semibold tabular-nums text-foreground">
          {fmtTokens(total)}
        </span>
      </div>
      <div className="space-y-0.5">
        {payload.map((p: any) => (
          <div key={String(p.dataKey)} className="flex items-center gap-2">
            <span
              className="h-2 w-2 shrink-0 rounded-full"
              style={{ backgroundColor: p.stroke ?? p.color }}
            />
            <span className="text-muted-foreground">{p.name}</span>
            <span className="ml-auto pl-4 font-medium tabular-nums text-foreground">
              {fmtTokens(Number(p.value) || 0)}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

type Granularity = "hour" | "day" | "month";

/** 跨度 → 粒度:≤2 天按小时,≤62 天按天,更长按月 */
function pickGranularity(from: string, to: string): Granularity {
  const days = dayjs(to).diff(dayjs(from.slice(0, 10)), "day") + 1;
  if (days <= 2) return "hour";
  if (days <= 62) return "day";
  return "month";
}

function minBucket(rows: DailyUsage[]): string | null {
  if (rows.length === 0) return null;
  let min = rows[0].date;
  for (const r of rows) if (r.date < min) min = r.date;
  return min;
}

function parseAxis(s: string): dayjs.Dayjs {
  if (s.length >= 16 && s[10] === " ") {
    return dayjs(`${s.slice(0, 10)}T${s.slice(11, 16)}:00`);
  }
  return dayjs(s);
}

function buildHourBuckets(from: string, to: string): string[] {
  const out: string[] = [];
  let cur = parseAxis(from).startOf("hour");
  // to 是日期字符串(当天 00:00),必须延伸到当天 23 时,否则只生成一个桶
  const end = dayjs(to).endOf("day").startOf("hour");
  while ((cur.isBefore(end) || cur.isSame(end, "hour")) && out.length < 240) {
    out.push(cur.format("YYYY-MM-DD HH:00"));
    cur = cur.add(1, "hour");
  }
  return out;
}

function buildMonthBuckets(from: string, to: string): string[] {
  const out: string[] = [];
  let cur = dayjs(from).startOf("month");
  const end = dayjs(to).startOf("month");
  while (cur.isBefore(end) || cur.isSame(end, "month")) {
    out.push(cur.format("YYYY-MM"));
    cur = cur.add(1, "month");
  }
  return out;
}

function buildDayBuckets(from: string, to: string): string[] {
  const out: string[] = [];
  let cur = dayjs(from).startOf("day");
  const end = dayjs(to).startOf("day");
  while ((cur.isBefore(end) || cur.isSame(end, "day")) && out.length < MAX_DAYS) {
    out.push(cur.format("YYYY-MM-DD"));
    cur = cur.add(1, "day");
  }
  return out;
}

/** 目标约 8 个刻度;interval=n 表示每隔 n 个桶显示一个 */
function pickTickInterval(count: number): number {
  if (count <= 8) return 0;
  return Math.max(0, Math.ceil(count / 8) - 1);
}

function formatRangeCaption(start: string, to: string, all: boolean): string {
  const fromDay = start.slice(0, 10);
  if (all && fromDay.startsWith("2000-")) return "全部历史";
  if (fromDay === to) return to;
  if (all || fromDay.slice(0, 4) !== to.slice(0, 4)) return `${fromDay} ~ ${to}`;
  return `${fromDay.slice(5)} ~ ${to.slice(5)}`;
}

interface TrendChartProps {
  agentId: string | null;
  from: string;
  to: string;
  /** 全部时间:轴从该筛选下最早记录起,刻度按「最早记录 → 现在」自适应 */
  all?: boolean;
  refreshKey: number | string;
}

/** 各 Agent 消耗趋势:默认对比模式(各自从 0 起,数量级悬殊时也一目了然),可切堆叠;
 *  刻度随范围自适应(≤2天按小时 / ≤62天按天 / 更长按月);
 *  选「全部」时 X 轴从该 Agent(或全部 Agent)最早记录拉到今天,不用哨兵日期 2000-01-01 */
export function TrendChart({
  agentId,
  from,
  to,
  all = false,
  refreshKey,
}: TrendChartProps) {
  const [stacked, setStacked] = useState(false);
  const [rows, setRows] = useState<DailyUsage[] | null>(null);
  const [granularity, setGranularity] = useState<Granularity>(() =>
    pickGranularity(from, to),
  );
  /** 轴起点:全部时间用最早记录,其余用筛选 from */
  const [axisFrom, setAxisFrom] = useState(from);

  useEffect(() => {
    let active = true;

    const load = async () => {
      try {
        if (all) {
          const probe = await api.getDaily(agentId, from, to, "day");
          if (!active) return;
          const earliest = minBucket(probe);
          if (!earliest) {
            setGranularity("day");
            setAxisFrom(from);
            setRows([]);
            return;
          }
          const g = pickGranularity(earliest, to);
          setGranularity(g);
          setAxisFrom(earliest);
          if (g === "day") {
            setRows(probe);
            return;
          }
          const data = await api.getDaily(agentId, earliest, to, g);
          if (!active) return;
          if (g === "hour") {
            setAxisFrom(minBucket(data) ?? earliest);
          }
          setRows(data);
          return;
        }

        const g = pickGranularity(from, to);
        setGranularity(g);
        setAxisFrom(from);
        const data = await api.getDaily(agentId, from, to, g);
        if (active) setRows(data);
      } catch (err) {
        console.error("[TrendChart] getDaily 失败", err);
        if (active) setRows([]);
      }
    };

    void load();
    return () => {
      active = false;
    };
  }, [agentId, from, to, all, refreshKey]);

  /** 出现过数据的 agent,按已知顺序排列 */
  const agentIds = useMemo(() => {
    const ids = new Set<string>();
    for (const r of rows ?? []) ids.add(r.agent);
    return [...ids].sort((a, b) => {
      const ia = KNOWN_ORDER.indexOf(a);
      const ib = KNOWN_ORDER.indexOf(b);
      if (ia !== -1 && ib !== -1) return ia - ib;
      if (ia !== -1) return -1;
      if (ib !== -1) return 1;
      return a.localeCompare(b);
    });
  }, [rows]);

  /** 连续桶轴。全部时间:从最早记录铺到今天;其余小时/月用筛选范围,天用数据实际范围 */
  const buckets = useMemo<string[]>(() => {
    if (granularity === "hour") {
      return buildHourBuckets(all ? axisFrom : from, to);
    }
    if (granularity === "month") {
      return buildMonthBuckets(all ? axisFrom : from, to);
    }
    if (all) {
      if (!axisFrom || axisFrom.startsWith("2000-")) return [];
      return buildDayBuckets(axisFrom, to);
    }
    const set = new Set((rows ?? []).map((r) => r.date));
    if (set.size === 0) return [];
    const sorted = [...set].sort();
    return buildDayBuckets(sorted[0], sorted[sorted.length - 1]);
  }, [rows, from, to, axisFrom, granularity, all]);

  /** pivot 成 [{date: 桶键, [agentId]: totalTokens}],按桶轴补零 */
  const data = useMemo<PivotRow[]>(() => {
    const map = new Map<string, PivotRow>();
    for (const r of rows ?? []) {
      const row = map.get(r.date) ?? { date: r.date };
      row[r.agent] = ((row[r.agent] as number) ?? 0) + r.totalTokens;
      map.set(r.date, row);
    }
    return buckets.map((bucket) => {
      const row = map.get(bucket) ?? { date: bucket };
      for (const id of agentIds) {
        if (row[id] == null) row[id] = 0;
      }
      return row;
    });
  }, [rows, buckets, agentIds]);

  const axisStartDay = axisFrom.slice(0, 10);
  const crossYear = axisStartDay.slice(0, 4) !== to.slice(0, 4);
  const multiDayHours =
    granularity === "hour" &&
    dayjs(axisFrom.slice(0, 10)).format("YYYY-MM-DD") !== to;
  const fmtTick = (v: any) => {
    const s = String(v);
    if (granularity === "hour") {
      return multiDayHours ? `${s.slice(5, 10)} ${s.slice(11, 16)}` : s.slice(11, 16);
    }
    if (granularity === "month") return s;
    return crossYear ? s : s.slice(5);
  };
  const granularityLabel =
    granularity === "hour" ? "按小时" : granularity === "month" ? "按月" : "按天";
  const tickInterval = pickTickInterval(buckets.length);

  return (
    <section className="rounded-xl border border-border bg-card p-4 transition-all duration-300 hover:border-primary/60 hover:shadow-sm">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-1.5 text-sm font-semibold">
          <TrendingUpIcon className="h-4 w-4 text-primary" />
          <span>消耗趋势</span>
          <span className="ml-1 text-xs font-normal text-muted-foreground">
            {formatRangeCaption(all ? axisFrom : from, to, all)} ·{" "}
            {granularityLabel} · 各 Agent Token 总量
          </span>
        </div>
        <div className="flex items-center gap-1 rounded-xl bg-muted p-1">
          {(
            [
              { v: false, label: "对比" },
              { v: true, label: "堆叠" },
            ] as const
          ).map(({ v, label }) => (
            <button
              key={label}
              type="button"
              onClick={() => setStacked(v)}
              title={
                v
                  ? "各 Agent 从上往下叠出总量"
                  : "各 Agent 独立画线,便于横向对比"
              }
              className={`h-7 rounded-lg px-2.5 text-xs font-medium transition-colors ${
                stacked === v
                  ? "bg-background shadow-sm text-foreground"
                  : "text-muted-foreground hover:bg-black/5 dark:hover:bg-white/5"
              }`}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      {rows === null ? (
        <Skeleton className="mt-3 h-64" />
      ) : rows.length === 0 || buckets.length === 0 ? (
        <div className="mt-3">
          <EmptyState message="所选范围内暂无消耗数据" />
        </div>
      ) : (
        <>
          <div className="mt-3 h-64 w-full">
            <ResponsiveContainer width="100%" height="100%">
              <AreaChart
                data={data}
                margin={{ top: 10, right: 8, left: 0, bottom: 0 }}
              >
                <defs>
                  {agentIds.map((id) => (
                    <linearGradient
                      key={id}
                      id={`trend-grad-${id}`}
                      x1="0"
                      y1="0"
                      x2="0"
                      y2="1"
                    >
                      <stop
                        offset="0%"
                        stopColor={agentColor(id)}
                        stopOpacity={0.25}
                      />
                      <stop
                        offset="100%"
                        stopColor={agentColor(id)}
                        stopOpacity={0}
                      />
                    </linearGradient>
                  ))}
                </defs>
                <CartesianGrid
                  strokeDasharray="3 3"
                  vertical={false}
                  stroke="hsl(var(--border))"
                  strokeOpacity={0.4}
                />
                <XAxis
                  dataKey="date"
                  axisLine={false}
                  tickLine={false}
                  dy={8}
                  interval={tickInterval}
                  minTickGap={16}
                  tick={{ fill: "hsl(var(--muted-foreground))", fontSize: 12 }}
                  tickFormatter={fmtTick}
                />
                <YAxis
                  axisLine={false}
                  tickLine={false}
                  width={44}
                  tick={{ fill: "hsl(var(--muted-foreground))", fontSize: 12 }}
                  tickFormatter={(v: any) => fmtTokens(Number(v))}
                />
                <Tooltip
                  content={
                    <TrendTooltip
                      granularity={granularity}
                      crossYear={crossYear}
                    />
                  }
                  wrapperStyle={{ zIndex: 50, pointerEvents: "none" }}
                />
                {agentIds.map((id) => (
                  <Area
                    key={id}
                    type="monotone"
                    dataKey={id}
                    name={agentLabel(id)}
                    stackId={stacked ? "a" : undefined}
                    stroke={agentColor(id)}
                    strokeWidth={1.5}
                    fill={`url(#trend-grad-${id})`}
                  />
                ))}
              </AreaChart>
            </ResponsiveContainer>
          </div>
          <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-1.5">
            {agentIds.map((id) => (
              <span
                key={id}
                className="flex items-center gap-1.5 text-xs text-muted-foreground"
              >
                <span
                  className="h-2 w-2 rounded-full"
                  style={{ backgroundColor: agentColor(id) }}
                />
                {agentLabel(id)}
              </span>
            ))}
          </div>
        </>
      )}
    </section>
  );
}
