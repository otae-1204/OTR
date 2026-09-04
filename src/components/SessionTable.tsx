import { useEffect, useState } from "react";
import {
  agentColor,
  AGENT_LABELS,
  api,
  type SessionUsage,
} from "../api/bindings";
import { fmtCost, fmtDateTime, fmtTokens, fromNow } from "../lib/format";
import { EmptyState, Skeleton } from "./Skeleton";
import { ClockIcon } from "./icons";

function agentLabel(id: string): string {
  return AGENT_LABELS[id] ?? id;
}

function tokenDetail(s: SessionUsage): string {
  return `输入 ${fmtTokens(s.inputTokens)} · 输出 ${fmtTokens(s.outputTokens)} · 缓存读 ${fmtTokens(s.cacheReadTokens)} · 缓存写 ${fmtTokens(s.cacheWriteTokens)} · ${s.calls} 次请求`;
}

interface SessionTableProps {
  agentId: string | null;
  /** null 表示不限(全部时间) */
  from: string | null;
  to: string | null;
  refreshKey: number | string;
  rangeLabel: string;
  currency: string;
  rate: number;
}

/** 会话明细,随筛选栏的范围与 Agent 联动 */
export function SessionTable({
  agentId,
  from,
  to,
  refreshKey,
  rangeLabel,
  currency,
  rate,
}: SessionTableProps) {
  const [sessions, setSessions] = useState<SessionUsage[] | null>(null);

  useEffect(() => {
    let active = true;
    setSessions(null);
    api
      .getSessions(agentId, from, to, 50)
      .then((rows) => {
        if (active) setSessions(rows);
      })
      .catch((err) => {
        console.error("[SessionTable] getSessions 失败", err);
        if (active) setSessions([]);
      });
    return () => {
      active = false;
    };
  }, [agentId, from, to, refreshKey]);

  return (
    <section className="rounded-xl border border-border bg-card p-4 transition-all duration-300 hover:border-primary/60 hover:shadow-sm">
      <div className="flex items-center gap-1.5 text-sm font-semibold">
        <ClockIcon className="h-4 w-4 text-primary" />
        <span>会话明细</span>
        <span className="ml-1 text-xs font-normal text-muted-foreground">
          {rangeLabel} · 最近 {sessions?.length ?? "--"} 条
        </span>
      </div>

      <div className="mt-3">
        {sessions === null ? (
          <Skeleton className="h-56" />
        ) : sessions.length === 0 ? (
          <EmptyState message="筛选范围内暂无会话记录" />
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[720px] text-sm">
              <thead>
                <tr className="border-b border-border/60 text-left text-xs text-muted-foreground">
                  <th className="py-2 pr-3 font-medium">最后活跃</th>
                  <th className="py-2 pr-3 font-medium">Agent</th>
                  <th className="py-2 pr-3 font-medium">项目 / 标题</th>
                  <th className="py-2 pr-3 font-medium">模型</th>
                  <th className="py-2 pr-3 text-right font-medium">Tokens</th>
                  <th className="py-2 text-right font-medium">成本</th>
                </tr>
              </thead>
              <tbody>
                {sessions.map((s, i) => {
                  const title =
                    s.project ||
                    s.title ||
                    (s.sessionId ? s.sessionId.slice(0, 8) : "--");
                  const models = (s.models ?? "")
                    .split(",")
                    .map((m) => m.trim())
                    .filter(Boolean)
                    .join(", ");
                  const tooltipTitle =
                    [s.project, s.title].filter(Boolean).join(" · ") ||
                    undefined;
                  return (
                    <tr
                      key={`${s.sessionId ?? "row"}-${i}`}
                      className="border-b border-border/40 transition-colors last:border-0 hover:bg-muted/30"
                    >
                      <td
                        className="whitespace-nowrap py-2.5 pr-3 text-muted-foreground"
                        title={
                          s.lastActive != null
                            ? fmtDateTime(s.lastActive)
                            : undefined
                        }
                      >
                        {fromNow(s.lastActive)}
                      </td>
                      <td className="whitespace-nowrap py-2.5 pr-3">
                        <span className="flex items-center gap-1.5">
                          <span
                            className="h-2 w-2 shrink-0 rounded-full"
                            style={{ backgroundColor: agentColor(s.agent) }}
                          />
                          {agentLabel(s.agent)}
                        </span>
                      </td>
                      <td
                        className="max-w-[220px] truncate py-2.5 pr-3"
                        title={tooltipTitle}
                      >
                        {title}
                      </td>
                      <td
                        className="max-w-[180px] truncate py-2.5 pr-3 text-xs text-muted-foreground"
                        title={models || undefined}
                      >
                        {models || "--"}
                      </td>
                      <td
                        className="py-2.5 pr-3 text-right font-semibold tabular-nums"
                        title={tokenDetail(s)}
                      >
                        {fmtTokens(s.totalTokens)}
                      </td>
                      <td className="py-2.5 text-right tabular-nums text-green-500">
                        {fmtCost(s.cost, currency, rate)}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </section>
  );
}
