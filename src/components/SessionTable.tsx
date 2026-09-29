import { useEffect, useRef, useState } from "react";
import { AGENT_LABELS, api, type SessionUsage } from "../api/bindings";
import { useTheme } from "../theme/ThemeProvider";
import { fmtCost, fmtDateTime, fmtTokens, fromNow } from "../lib/format";
import { EmptyState, Skeleton } from "./Skeleton";
import { ClockIcon } from "./icons";

function agentLabel(id: string): string {
  return AGENT_LABELS[id] ?? id;
}

function tokenDetail(s: SessionUsage): string {
  return `输入 ${fmtTokens(s.inputTokens)} · 输出 ${fmtTokens(s.outputTokens)} · 缓存读 ${fmtTokens(s.cacheReadTokens)} · 缓存写 ${fmtTokens(s.cacheWriteTokens)} · ${s.calls} 次请求`;
}

function nonempty(value: string | null | undefined): string | null {
  const text = value?.trim();
  return text ? text : null;
}

/** 会话名优先:Cursor 的 title 会被换成 composer 的 name,DSH/OpenCode 的 title 本来就是会话名。
 *  没有名字时才退回项目路径,不再截一段 session_id 来充数。 */
function sessionName(s: SessionUsage): string {
  return nonempty(s.title) || nonempty(s.project) || "—";
}

function sessionTooltip(s: SessionUsage): string | undefined {
  const parts = [nonempty(s.title), nonempty(s.project)].filter(
    (part, index, all): part is string =>
      !!part && all.indexOf(part) === index,
  );
  return parts.length > 0 ? parts.join(" · ") : undefined;
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
  const { agentColor } = useTheme();
  const [sessions, setSessions] = useState<SessionUsage[] | null>(null);
  const requestId = useRef(0);
  const filterKey = `${agentId ?? ""}|${from ?? ""}|${to ?? ""}`;
  const filterRef = useRef(filterKey);

  useEffect(() => {
    const current = ++requestId.current;
    let active = true;
    const filterChanged = filterRef.current !== filterKey;
    filterRef.current = filterKey;
    // 同一筛选下的数据更新留在原地替换,不要先换成骨架屏。
    // 每次清空再重绘会让整张表闪一下,行高和数字都会跳。
    if (filterChanged) setSessions(null);
    api
      .getSessions(agentId, from, to, 50)
      .then((rows) => {
        if (active && requestId.current === current) setSessions(rows);
      })
      .catch((err) => {
        console.error("[SessionTable] getSessions 失败", err);
        if (active && requestId.current === current) {
          setSessions((prev) => prev ?? []);
        }
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
                  <th className="py-2 pr-3 font-medium">会话</th>
                  <th className="py-2 pr-3 font-medium">模型</th>
                  <th className="py-2 pr-3 text-right font-medium">Tokens</th>
                  <th className="py-2 text-right font-medium">成本</th>
                </tr>
              </thead>
              <tbody>
                {sessions.map((s, i) => {
                  const name = sessionName(s);
                  const models = (s.models ?? "")
                    .split(",")
                    .map((m) => m.trim())
                    .filter(Boolean)
                    .join(", ");
                  const tooltipTitle = sessionTooltip(s);
                  return (
                    <tr
                      key={s.sessionId ? `${s.agent}:${s.sessionId}` : `row-${i}`}
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
                        {name}
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
                      <td className="py-2.5 text-right tabular-nums text-stat-cost">
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
