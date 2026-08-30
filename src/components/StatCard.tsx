import type { ReactNode } from "react";
import type { RangeSummary, Totals } from "../api/bindings";
import { fmtCost, fmtDateTime, fmtTokens } from "../lib/format";
import {
  ActivityIcon,
  ArrowDownIcon,
  ArrowUpIcon,
  CoinsIcon,
  DatabaseIcon,
  DownloadIcon,
} from "./icons";

function MiniStat({
  icon,
  label,
  value,
  accent,
  title,
}: {
  icon: ReactNode;
  label: string;
  value: string;
  /** 语义色 class,如 text-blue-500 */
  accent: string;
  title?: string;
}) {
  return (
    <div
      className="flex flex-col gap-1 rounded-xl border border-border/40 bg-background/40 p-3"
      title={title}
    >
      <div
        className={`flex items-center gap-1.5 text-[11px] font-medium ${accent}`}
      >
        {icon}
        <span className="tracking-wide">{label}</span>
      </div>
      <div className="text-sm font-semibold tabular-nums">{value}</div>
    </div>
  );
}

/** 缓存命中率 = 缓存读 / (未缓存输入 + 缓存读) */
export function cacheHitRate(t: Totals): number {
  const denom = t.inputTokens + t.cacheReadTokens;
  return denom > 0 ? t.cacheReadTokens / denom : 0;
}

const HIT_RATE_TIP =
  "缓存命中率 = 缓存读 ÷ (未缓存输入 + 缓存读)。「缓存读」是命中上下文缓存的输入部分:多轮对话里系统提示、代码上下文等每轮都从缓存读取,所以缓存读经常比未缓存输入还高,是正常现象。";

interface StatCardProps {
  summary: RangeSummary | null;
  /** 主标题,如 "近30天 · Codex CLI" */
  title: string;
  /** 范围说明副标题,如 "统计 2026-07-31 ~ 2026-08-29 期间的用量" */
  subtitle: string;
  /** 数据加载中 */
  loading: boolean;
  /** 当前选中 Agent 的全部时间累计 tokens(给"今日为 0"提供参照),null 表示未选 Agent */
  agentAllTimeTokens: number | null;
  currency: string;
  rate: number;
}

/** Hero 统计卡:由筛选栏的 Agent + 日期范围驱动 */
export function StatCard({
  summary,
  title,
  subtitle,
  loading,
  agentAllTimeTokens,
  currency,
  rate,
}: StatCardProps) {
  const totals: Totals | null = summary?.totals ?? null;
  const hit = totals ? cacheHitRate(totals) : 0;
  const hitPct = (hit * 100).toFixed(1);

  return (
    <section className="rounded-xl border border-border bg-card p-4 transition-all duration-300 hover:border-primary/60 hover:shadow-sm">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <ActivityIcon className="h-3.5 w-3.5 text-primary" />
            <span>{title}</span>
          </div>
          <p className="mt-1 text-xs text-muted-foreground/80">{subtitle}</p>
          <div className="mt-2 flex items-baseline gap-2">
            <span
              className="text-3xl font-bold tabular-nums tracking-tight leading-none"
              title={totals ? `${totals.totalTokens.toLocaleString()} tokens` : undefined}
            >
              {totals ? fmtTokens(totals.totalTokens) : "--"}
            </span>
            {totals ? (
              <span className="rounded-md bg-muted/40 px-1.5 py-0.5 text-xs text-muted-foreground">
                {totals.calls.toLocaleString()} 次请求
              </span>
            ) : null}
            {loading ? (
              <span className="animate-pulse text-xs text-muted-foreground">
                刷新中…
              </span>
            ) : null}
          </div>
        </div>
        <div className="flex flex-col items-end gap-1.5">
          {summary ? (
            <span
              className="rounded-md bg-muted/40 px-1.5 py-0.5 text-xs text-muted-foreground"
              title="数据生成时间"
            >
              更新于 {fmtDateTime(summary.generatedAt)}
            </span>
          ) : null}
          {agentAllTimeTokens != null ? (
            <span className="text-xs text-muted-foreground">
              该 Agent 全部时间累计{" "}
              <span className="font-semibold tabular-nums text-foreground">
                {fmtTokens(agentAllTimeTokens)}
              </span>
            </span>
          ) : null}
        </div>
      </div>

      {/* 缓存命中率进度条 */}
      <div className="mt-3" title={HIT_RATE_TIP}>
        <div className="flex items-center justify-between text-[11px] text-muted-foreground">
          <span className="flex items-center gap-1.5 font-medium text-emerald-500">
            <DatabaseIcon className="h-3 w-3" />
            缓存命中率
          </span>
          <span className="font-semibold tabular-nums">{hitPct}%</span>
        </div>
        <div className="relative mt-1 h-1.5 overflow-hidden rounded-full bg-muted/60">
          <div
            className="absolute inset-y-0 left-0 rounded-full bg-emerald-500 transition-all duration-500"
            style={{ width: `${(hit * 100).toFixed(1)}%` }}
          />
        </div>
      </div>

      <div className="mt-3 grid grid-cols-3 gap-3 lg:grid-cols-6">
        <MiniStat
          icon={<ArrowDownIcon className="h-3.5 w-3.5" />}
          label="输入"
          accent="text-blue-500"
          value={totals ? fmtTokens(totals.inputTokens) : "--"}
          title={
            totals
              ? `未命中缓存的输入 ${totals.inputTokens.toLocaleString()} tokens`
              : undefined
          }
        />
        <MiniStat
          icon={<ArrowUpIcon className="h-3.5 w-3.5" />}
          label="输出"
          accent="text-purple-500"
          value={totals ? fmtTokens(totals.outputTokens) : "--"}
          title={totals ? `${totals.outputTokens.toLocaleString()} tokens` : undefined}
        />
        <MiniStat
          icon={<DatabaseIcon className="h-3.5 w-3.5" />}
          label="缓存读"
          accent="text-emerald-500"
          value={totals ? fmtTokens(totals.cacheReadTokens) : "--"}
          title={
            totals
              ? `命中上下文缓存的输入 ${totals.cacheReadTokens.toLocaleString()} tokens(可能高于未缓存输入,属正常)`
              : undefined
          }
        />
        <MiniStat
          icon={<DownloadIcon className="h-3.5 w-3.5" />}
          label="缓存写"
          accent="text-amber-500"
          value={totals ? fmtTokens(totals.cacheWriteTokens) : "--"}
          title={totals ? `${totals.cacheWriteTokens.toLocaleString()} tokens` : undefined}
        />
        <MiniStat
          icon={<ActivityIcon className="h-3.5 w-3.5" />}
          label="请求次数"
          accent="text-sky-500"
          value={totals ? totals.calls.toLocaleString() : "--"}
        />
        <MiniStat
          icon={<CoinsIcon className="h-3.5 w-3.5" />}
          label="成本"
          accent="text-green-500"
          value={totals ? fmtCost(totals.cost, currency, rate) : "--"}
          title={
            totals
              ? `成本 ${totals.cost.toFixed(4)}(填了定价的模型按定价重算,其余用自带成本或 0)`
              : undefined
          }
        />
      </div>
    </section>
  );
}
