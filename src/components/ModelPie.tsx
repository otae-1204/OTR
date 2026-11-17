import { useMemo } from "react";
import { Cell, Pie, PieChart, ResponsiveContainer, Tooltip } from "recharts";
import type { ModelSlice, RangeSummary, Totals } from "../api/bindings";
import { fmtCost, fmtTokens, fmtTps, fmtTtft, tpsTitle } from "../lib/format";
import { cacheHitRate } from "./StatCard";
import { EmptyState } from "./Skeleton";
import { PieChartIcon } from "./icons";
import { useTheme } from "../theme/ThemeProvider";


interface Slice {
  name: string;
  value: number;
  /** 渲染用值:极小占比钳到可见角度,真实值在 tooltip/图例展示 */
  renderValue: number;
  speed: string | null;
}

function PieTooltip({ active, payload, total }: any) {
  if (!active || !payload || payload.length === 0) return null;
  const p = payload[0];
  const value = Number(p.payload?.real ?? p.value) || 0;
  const name = String(p.name ?? "--");
  const pct = total > 0 ? ((value / total) * 100).toFixed(1) : "0.0";
  return (
    <div
      data-theme-part="tooltip"
      className="rounded-lg border border-border bg-card px-3 py-2 text-xs shadow-lg"
    >
      <p className="mb-0.5 max-w-[220px] truncate font-medium text-foreground" title={name}>
        {name}
      </p>
      <p className="tabular-nums text-muted-foreground">
        {fmtTokens(value)} tokens · {pct}%
        {p.payload?.speed ? ` · ${p.payload.speed}` : ""}
      </p>
    </div>
  );
}

function HitRateBar({ totals }: { totals: Totals }) {
  const hit = cacheHitRate(totals);
  return (
    <div
      title="缓存读 ÷ (未缓存输入 + 缓存读)"
      className="flex items-center gap-2"
    >
      <div data-theme-part="progress" className="relative h-1.5 w-16 overflow-hidden rounded-full bg-muted/60">
        <div
          data-theme-part="progress-fill"
          className="absolute inset-y-0 left-0 rounded-full bg-success"
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
  /** 当前点中的扇区,含「其他」;null = 没选 */
  selected: string | null;
  onSelect: (name: string | null) => void;
}

/** 模型占比(环形图 + 图例);点击扇区在该卡片内查看该模型明细(含缓存命中率) */
export function ModelPie({
  summary,
  rangeLabel,
  currency,
  rate,
  selected,
  onSelect,
}: ModelPieProps) {
  // 扇区按序取色,调色板来自当前主题
  const { chartPalette: PALETTE } = useTheme();

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
      speed: fmtTps(s.totals.genOutputTokens, s.totals.genMs)
        ?? fmtTps(s.totals.estOutputTokens, s.totals.estMs),
    }));
    if (rest.length > 0) {
      built.push({
        name: "其他",
        value: rest.reduce((acc, s) => acc + s.totals.totalTokens, 0),
        renderValue: 0,
        speed: fmtTps(
          rest.reduce((acc, s) => acc + s.totals.genOutputTokens, 0),
          rest.reduce((acc, s) => acc + s.totals.genMs, 0),
        ) ?? fmtTps(
          rest.reduce((acc, s) => acc + s.totals.estOutputTokens, 0),
          rest.reduce((acc, s) => acc + s.totals.estMs, 0),
        ),
      });
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
          genOutputTokens: acc.genOutputTokens + s.totals.genOutputTokens,
          genMs: acc.genMs + s.totals.genMs,
          genSamples: acc.genSamples + s.totals.genSamples,
          estOutputTokens: acc.estOutputTokens + s.totals.estOutputTokens,
          estMs: acc.estMs + s.totals.estMs,
          estSamples: acc.estSamples + s.totals.estSamples,
          ttftMs: acc.ttftMs + s.totals.ttftMs,
          ttftSamples: acc.ttftSamples + s.totals.ttftSamples,
        }),
        {
          inputTokens: 0,
          outputTokens: 0,
          cacheReadTokens: 0,
          cacheWriteTokens: 0,
          calls: 0,
          cost: 0,
          totalTokens: 0,
          genOutputTokens: 0,
          genMs: 0,
          genSamples: 0,
          estOutputTokens: 0,
          estMs: 0,
          estSamples: 0,
          ttftMs: 0,
          ttftSamples: 0,
        },
      );
    return { name: "其他", totals: restTotal as Totals };
  }, [selected, summary]);

  const total = slices.reduce((acc, s) => acc + s.value, 0);
  const totalPct = (v: number) =>
    total > 0 ? ((v / total) * 100).toFixed(1) : "0.0";
  const selectedReal = selectedSlice
    ? fmtTps(selectedSlice.totals.genOutputTokens, selectedSlice.totals.genMs)
    : null;
  const selectedEst = selectedSlice
    ? fmtTps(selectedSlice.totals.estOutputTokens, selectedSlice.totals.estMs)
    : null;
  const selectedSpeed = selectedReal ?? selectedEst;
  const selectedSpeedLabel = selectedReal ? "平均速度" : "估算速度";
  const selectedTtft =
    selectedSlice && selectedSlice.totals.ttftSamples > 0
      ? fmtTtft(selectedSlice.totals.ttftMs, selectedSlice.totals.ttftSamples)
      : null;

  return (
    <section
      data-theme-part="card chart-card"
      className="pie-card flex h-full flex-col rounded-xl border border-border bg-card p-4 transition-all duration-300 hover:border-primary/60 hover:shadow-sm"
    >
      <div data-theme-part="card-title" className="flex items-center gap-1.5 text-sm font-semibold">
        <PieChartIcon className="h-4 w-4 text-primary" />
        <span>模型占比</span>
        <span className="ml-1 text-xs font-normal text-muted-foreground">
          {rangeLabel} · 按模型 Token 分布
        </span>
      </div>

      {slices.length === 0 ? (
        <div className="mt-3 flex-1">
          <EmptyState message="所选范围内暂无模型数据" />
        </div>
      ) : (
        <div className="mt-3 flex flex-1 flex-col items-center gap-4 lg:flex-row">
          <div data-theme-part="chart" className="relative h-[200px] w-full max-w-[210px] shrink-0">
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
                  innerRadius={58}
                  outerRadius={88}
                  paddingAngle={slices.length > 1 ? 2 : 0}
                  strokeWidth={2}
                  stroke="hsl(var(--card))"
                  animationDuration={400}
                  onClick={(entry: any) => {
                    const name = entry?.name ?? entry?.payload?.name;
                    if (typeof name === "string") {
                      onSelect(selected === name ? null : name);
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
                      style={{ transition: "fill-opacity 220ms ease" }}
                    />
                  ))}
                </Pie>
              </PieChart>
            </ResponsiveContainer>
            <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center px-4 text-center">
              <span className="max-w-full truncate text-10px text-muted-foreground">
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
            {selectedSlice ? (
              <div className="space-y-2.5 animate-fade-soft">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <button
                    type="button"
                    onClick={() => onSelect(null)}
                    className="inline-flex items-center gap-1 text-xs font-medium text-primary transition-colors hover:underline"
                  >
                    ← 全部模型
                  </button>
                  <HitRateBar totals={selectedSlice.totals} />
                </div>

                <div className="flex items-baseline justify-between gap-2">
                  <div
                    className="min-w-0 truncate text-sm font-semibold text-foreground"
                    title={selectedSlice.name}
                  >
                    {selectedSlice.name}
                  </div>
                  <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
                    占比 {totalPct(selectedSlice.totals.totalTokens)}%
                  </span>
                </div>

                {selectedSpeed || selectedTtft ? (
                  <div className="flex flex-wrap items-baseline gap-x-3 gap-y-0.5 text-11px tabular-nums text-muted-foreground">
                    {selectedSpeed ? (
                      <span
                        title={
                          selectedReal
                            ? tpsTitle(
                                selectedSlice.totals.genOutputTokens,
                                selectedSlice.totals.genMs,
                                selectedSlice.totals.genSamples,
                              )
                            : selectedEst
                              ? tpsTitle(
                                  selectedSlice.totals.estOutputTokens,
                                  selectedSlice.totals.estMs,
                                  selectedSlice.totals.estSamples,
                                )
                              : undefined
                        }
                      >
                        {selectedSpeedLabel}{" "}
                        <span className="font-semibold text-foreground">
                          {selectedSpeed}
                        </span>
                      </span>
                    ) : null}
                    {selectedTtft ? (
                      <span>
                        平均首字{" "}
                        <span className="font-semibold text-foreground">
                          {selectedTtft}
                        </span>
                      </span>
                    ) : null}
                  </div>
                ) : null}

                <div className="grid grid-cols-3 gap-1.5 text-xs">
                  {(
                    [
                      [
                        "输入",
                        fmtTokens(selectedSlice.totals.inputTokens),
                        `${selectedSlice.totals.inputTokens.toLocaleString()} tokens`,
                        "",
                      ],
                      [
                        "输出",
                        fmtTokens(selectedSlice.totals.outputTokens),
                        `${selectedSlice.totals.outputTokens.toLocaleString()} tokens`,
                        "",
                      ],
                      [
                        "缓存读",
                        fmtTokens(selectedSlice.totals.cacheReadTokens),
                        `${selectedSlice.totals.cacheReadTokens.toLocaleString()} tokens`,
                        "",
                      ],
                      [
                        "缓存写",
                        fmtTokens(selectedSlice.totals.cacheWriteTokens),
                        `${selectedSlice.totals.cacheWriteTokens.toLocaleString()} tokens`,
                        "",
                      ],
                      [
                        "请求",
                        selectedSlice.totals.calls.toLocaleString(),
                        undefined,
                        "",
                      ],
                      [
                        "成本",
                        fmtCost(selectedSlice.totals.cost, currency, rate),
                        "按定价估算",
                        "text-stat-cost",
                      ],
                    ] as const
                  ).map(([label, value, tip, valCls]) => (
                    <div
                      key={label}
                      data-theme-part="mini-stat"
                      title={tip}
                      className="min-w-0 rounded-lg border border-border/40 bg-background/60 px-2.5 py-2"
                    >
                      <div className="text-10px text-muted-foreground">{label}</div>
                      <div
                        className={`mt-0.5 truncate text-sm font-semibold tabular-nums ${valCls}`}
                      >
                        {value}
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            ) : (
              <ul data-theme-part="legend" className="space-y-1.5 animate-fade-soft">
                {slices.map((s, i) => (
                  <li
                    key={s.name}
                    role="button"
                    tabIndex={0}
                    onClick={() => onSelect(selected === s.name ? null : s.name)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        onSelect(selected === s.name ? null : s.name);
                      }
                    }}
                    className="flex cursor-pointer items-center gap-2 rounded-lg px-1.5 py-1 text-xs transition-colors hover:bg-muted/40"
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
            )}
          </div>
        </div>
      )}
    </section>
  );
}
