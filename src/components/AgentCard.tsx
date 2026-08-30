import { agentColor, type AgentStatus } from "../api/bindings";
import { fmtTokens } from "../lib/format";
import { AgentIcon, usesImageIcon } from "./AgentIcon";

interface AgentCardProps {
  status: AgentStatus;
  /** 所选日期范围内该 Agent 的 tokens(跟随筛选栏) */
  rangeTokens: number;
  /** 所选范围内全部 Agent 合计,用于计算占比 */
  rangeAllTokens: number;
  /** 范围文案,如 "今日" / "近30天" / "该范围" */
  rangeLabel: string;
  /** 当前是否为筛选选中的 Agent */
  selected: boolean;
  /** 点击切换选中;传 null 表示取消选中 */
  onSelect: (id: string | null) => void;
}

/** 单个 Agent 的用量卡片,大数字跟随筛选栏的日期范围;点击可只看该 Agent */
export function AgentCard({
  status,
  rangeTokens,
  rangeAllTokens,
  rangeLabel,
  selected,
  onSelect,
}: AgentCardProps) {
  const color = agentColor(status.id);
  const label = status.displayName || status.id;
  const pct =
    rangeAllTokens > 0
      ? Math.min(100, (rangeTokens / rangeAllTokens) * 100)
      : 0;
  const pctLabel = pct > 0 && pct < 1 ? "<1" : String(Math.round(pct));

  return (
    <button
      type="button"
      onClick={() => onSelect(selected ? null : status.id)}
      title={selected ? "点击取消筛选" : "点击只看这个 Agent"}
      className={`group rounded-xl border bg-card p-4 text-left transition-all duration-300 hover:border-primary/60 hover:shadow-sm ${
        selected
          ? "border-primary shadow-md"
          : "border-border"
      } ${status.enabled ? "" : "opacity-60"}`}
    >
      <div className="flex items-center gap-3">
        <div
          className={`flex h-9 w-9 shrink-0 items-center justify-center overflow-hidden rounded-lg border border-border/40`}
          style={{ backgroundColor: `${color}1A`, color }}
        >
          <AgentIcon
            id={status.id}
            color={color}
            className={
              usesImageIcon(status.id) ? "h-full w-full" : "h-5 w-5"
            }
          />
        </div>
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium">{label}</div>
          <div
            className="mt-0.5 text-xl font-bold tabular-nums tracking-tight leading-none"
            title={`${rangeLabel} ${rangeTokens.toLocaleString()} tokens`}
          >
            {fmtTokens(rangeTokens)}
          </div>
        </div>
      </div>

      <div className="mt-3 h-1.5 overflow-hidden rounded-full bg-muted/60">
        <div
          className="h-full rounded-full transition-all duration-500"
          style={{ width: `${pct}%`, backgroundColor: color }}
        />
      </div>

      <div className="mt-2 flex items-center justify-between text-xs text-muted-foreground">
        <span>
          占{rangeLabel} <span className="tabular-nums">{pctLabel}%</span>
        </span>
        <span title={`累计 ${status.totalTokens.toLocaleString()} tokens`}>
          累计 {fmtTokens(status.totalTokens)}
        </span>
      </div>
    </button>
  );
}
