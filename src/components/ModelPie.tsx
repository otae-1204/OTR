import { useMemo, useState } from "react";
import { Cell, Pie, PieChart, ResponsiveContainer, Tooltip } from "recharts";
import type { ModelSlice, RangeSummary, Totals } from "../api/bindings";
import { fmtCost, fmtTokens } from "../lib/format";
import { cacheHitRate } from "./StatCard";
import { EmptyState } from "./Skeleton";
import { PieChartIcon } from "./icons";

const PALETTE = [
  "#3b82f6",
  "#a855f7",
  "#10b981",
  "#f97316",
  "#f59e0b",
  "#06b6d4",
  "#ec4899",
  "#84cc16",
  "#64748b",
];

interface Slice {
  name: string;
  value: number;
  /** 渲染用值:极小占比钳到可见角度,真实值在 tooltip/图例展示 */
  renderValue: number;
}

function PieTooltip({ active, payload, total }: any) {
  if (!active || !payload || payload.length === 0) return null;
  const p = payload[0];
  const value = Number(p.payload?.real ?? p.value) || 0;
  const name = String(p.name ?? "--");
  const pct = total > 0 ? ((value / total) * 100).toFixed(1) : "0.0";
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
      <p className="mb-0.5 max-w-[220px] truncate font-medium text-foreground" title={name}>
        {name}
      </p>
      <p className="tabular-nums text-muted-foreground">
        {fmtTokens(value)} tokens · {pct}%
      </p>
    </div>
  );
}

function HitRateBar({ totals }: { totals: Totals }) {
  const hit = cacheHitRate(totals);
  return (
    <div
      title={`缓存命中率 = 缓存读 ÷ (未缓存输入 + 缓存读);缓存读高说明上下文复用得多,是省钱的好事`}
      className="flex items-center gap-2"
    >
      <div className="relative h-1.5 w-24 overflow-hidden rounded-full bg-muted/60">
        <div
          className="absolute inset-y-0 left-0 rounded-full bg-emerald-500"
          style={{ width: `${(hit * 100).toFixed(1)}%` }}
        />
      </div>
      <span className="text-xs tabular-nums text-muted-foreground">
        命中率 {(hit * 100).toFixed(1)}%
      </span>
    </div>
  );
}

interface ModelPieProps {
  summary: RangeSummary | null;
  /** 副标题里展示的范围文字 */
  rangeLabel: string;
  currency: string;
  rate: number;
}

/** 模型占比(环形图 + 图例);点击扇区在该卡片内查看该模型明细(含缓存命中率) */
export function ModelPie({ summary, rangeLabel, currency, rate }: ModelPieProps) {
  const [selected, setSelected] = useState<string | null>(null);

  const slices = useMemo<Slice[]>(() => {
    const list: ModelSlice[] = [...(summary?.byModel ?? [])].sort(
      (a, b) => b.totals.totalTokens - a.totals.totalTokens,
    );
    const items = list.slice(0, 8);
    const rest = list.slice(8);
    const built: Slice[] = items.map((s) => ({
      name: s.model,
      value: s.totals.totalTokens,
      renderValue: s.totals.totalTokens,
    }));
    if (rest.length > 0) {
      built.push({
        name: "其他",
        value: rest.reduce((acc, s) => acc + s.totals.totalTokens, 0),
        renderValue: built.length === 0 ? 0 : 0, // 先占位,下面统一钳制
      });
      built[built.length - 1] = {
        name: "其他",
        value: built[built.length - 1].value,
        renderValue: built[built.length - 1].value,
      };
    }
    const all = built.filter((s) => s.value > 0);
    const total = all.reduce((acc, s) => acc + s.value, 0);
    // 极小占比钳到可见角度(约 1%),真实值在 tooltip 与图例中展示
    const floor = total * 0.01;
    return all.map((s) => ({ ...s, renderValue: Math.max(s.value, floor) }));
  }, [summary]);

  // 选中模型的完整数据(前 8 之外的模型从完整列表里找)
  const selectedSlice = useMemo(() => {
    if (!selected || !summary) return null;
    const fromList = summary.byModel.find((s) => s.model === selected);
    if (fromList) return { name: selected, totals: fromList.totals };
    const restTotal = (summary.byModel ?? [])
      .slice(8)
      .reduce(
        (acc, s) => ({
          inputTokens: acc.inputTokens + s.totals.inputTokens,
          outputTokens: acc.outputTokens + s.totals.outputTokens,
          cacheReadTokens: acc.cacheReadTokens + s.totals.cacheReadTokens,
          cacheWriteTokens: acc.cacheWriteTokens + s.totals.cacheWriteTokens,
          calls: acc.calls + s.totals.calls,
          cost: acc.cost + s.totals.cost,
          totalTokens: acc.totalTokens + s.totals.totalTokens,
        }),
        {
          inputTokens: 0,
          outputTokens: 0,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          calls: 0,
          cost: 0,
          totalTokens: 0,
        },
      );
    return { name: "其他", totals: restTotal as Totals };
  }, [selected, summary]);

  const total = slices.reduce((acc, s) => acc + s.value, 0);
  const totalPct = (v: number) =>
    total > 0 ? ((v / total) * 100).toFixed(1) : "0.0";

  return (
    <section className="pie-card rounded-xl border border-border bg-card p-4 transition-all duration-300 hover:border-primary/60 hover:shadow-sm">
      <div className="flex items-center gap-1.5 text-sm font-semibold">
        <PieChartIcon className="h-4 w-4 text-primary" />
        <span>模型占比</span>
        <span className="ml-1 text-xs font-normal text-muted-foreground">
          {rangeLabel} · 按模型 Token 分布{selected ? " · 点击扇区查看明细" : ""}
        </span>
      </div>

      {slices.length === 0 ? (
        <div className="mt-3">
          <EmptyState message="所选范围内暂无模型数据" />
        </div>
      ) : (
        <>
        <div className="mt-3 flex flex-col items-center gap-4 lg:flex-row">
          <div className="relative h-[200px] w-full max-w-[220px] shrink-0">
            <ResponsiveContainer width="100%" height="100%">
              {/* 注意:recharts 的 Pie 点击扇区后会给 path 加焦点框,由 .pie-card 的 CSS 去掉 */}
              <PieChart>
                <Tooltip
                  content={<PieTooltip total={total} />}
                  allowEscapeViewBox={{ x: true, y: true }}
                  wrapperStyle={{
                    zIndex: 50,
                    overflow: "visible",
                    pointerEvents: "none",
                  }}
                />
                <Pie
                  data={slices}
                  dataKey="renderValue"
                  nameKey="name"
                  innerRadius={60}
                  outerRadius={90}
                  paddingAngle={slices.length > 1 ? 2 : 0}
                  strokeWidth={2}
                  stroke="hsl(var(--card))"
                  onClick={(entry: any) => {
                    const name = entry?.name ?? entry?.payload?.name;
                    if (typeof name === "string") {
                      setSelected((prev) => (prev === name ? null : name));
                    }
                  }}
                  style={{ cursor: "pointer" }}
                >
                  {slices.map((s, i) => (
                    <Cell
                      key={s.name}
                      fill={PALETTE[i % PALETTE.length]}
                      fillOpacity={selected && selected !== s.name ? 0.3 : 1}
                      stroke="hsl(var(--card))"
                      strokeWidth={2}
                    />
                  ))}
                </Pie>
              </PieChart>
            </ResponsiveContainer>
            <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center">
              <span className="text-[10px] text-muted-foreground">
                {selected ?? rangeLabel}
              </span>
              <span className="text-lg font-bold tabular-nums tracking-tight">
                {fmtTokens(
                  selected
                    ? (slices.find((s) => s.name === selected)?.value ?? 0)
                    : total,
                )}
              </span>
            </div>
          </div>

          <div className="w-full min-w-0 flex-1">
            <ul className="space-y-1.5">
              {slices.map((s, i) => (
                <li
                  key={s.name}
                  role="button"
                  tabIndex={0}
                  onClick={() =>
                    setSelected((prev) => (prev === s.name ? null : s.name))
                  }
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      setSelected((prev) => (prev === s.name ? null : s.name));
                    }
                  }}
                  className={`flex cursor-pointer items-center gap-2 rounded-lg px-1.5 py-1 text-xs transition-colors hover:bg-muted/40 ${
                    selected === s.name ? "bg-primary/10 ring-1 ring-primary/40" : ""
                  }`}
                >
                  <span
                    className="h-2 w-2 shrink-0 rounded-full"
                    style={{ backgroundColor: PALETTE[i % PALETTE.length] }}
                  />
                  <span
                    className="min-w-0 flex-1 truncate text-foreground/80"
                    title={s.name}
                  >
                    {s.name}
                  </span>
                  <span className="tabular-nums text-muted-foreground">
                    {fmtTokens(s.value)}
                  </span>
                  <span className="w-12 text-right tabular-nums text-muted-foreground">
                    {totalPct(s.value)}%
                  </span>
                </li>
              ))}
            </ul>

          </div>
        </div>

        {/* 选中模型的明细面板:整行铺开,不再挤在图例下方 */}
        {selectedSlice ? (
          <div className="mt-3 space-y-3 rounded-xl border border-border/50 bg-background/40 p-4 animate-fade-in">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span
                className="min-w-0 truncate text-base font-semibold"
                title={selectedSlice.name}
              >
                {selectedSlice.name}
              </span>
              <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
                占比 {totalPct(selectedSlice.totals.totalTokens)}% · 成本{" "}
                {fmtCost(selectedSlice.totals.cost, currency, rate)}
              </span>
            </div>
            <HitRateBar totals={selectedSlice.totals} />
            <div className="grid grid-cols-5 gap-2 text-xs">
              {(
                [
                  ["输入", selectedSlice.totals.inputTokens],
                  ["输出", selectedSlice.totals.outputTokens],
                  ["缓存读", selectedSlice.totals.cacheReadTokens],
                  ["缓存写", selectedSlice.totals.cacheWriteTokens],
                  ["请求", selectedSlice.totals.calls],
                ] as const
              ).map(([label, v]) => (
                <div
                  key={label}
                  className="rounded-lg border border-border/40 bg-background/60 px-3 py-2.5"
                >
                  <div className="text-[11px] text-muted-foreground">
                    {label}
                  </div>
                  <div className="mt-0.5 text-base font-semibold tabular-nums">
                    {label === "请求" ? v.toLocaleString() : fmtTokens(v)}
                  </div>
                </div>
              ))}
            </div>
          </div>
        ) : null}
        </>
      )}
    </section>
  );
}
