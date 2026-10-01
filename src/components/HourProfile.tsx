import { useEffect, useRef, useState } from "react";
import { Bar, BarChart, CartesianGrid, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { api } from "../api/bindings";
import { fmtTokens } from "../lib/format";
import { EmptyState, Skeleton } from "./Skeleton";
import { ClockIcon } from "./icons";

interface HourRow {
  hour: number;
  totalTokens: number;
}

function HourTooltip({ active, payload }: any) {
  if (!active || !payload || payload.length === 0) return null;
  const hour = Number(payload[0]?.payload?.hour);
  const value = Number(payload[0]?.value) || 0;
  const label = Number.isFinite(hour) ? `${String(hour).padStart(2, "0")}:00` : "--";
  return (
    <div
      data-theme-part="tooltip"
      className="rounded-lg border border-border bg-card px-3 py-2 text-xs shadow-lg"
    >
      <div className="flex items-center justify-between gap-4">
        <span className="font-medium text-foreground">{label}</span>
        <span className="font-semibold tabular-nums text-foreground">{fmtTokens(value)}</span>
      </div>
    </div>
  );
}

interface HourProfileProps {
  agentId: string | null;
  from: string;
  to: string;
  refreshKey: number | string;
  model?: string | null;
  modelColor?: string | null;
}

/** 把范围内各天的同一个钟点加在一起。只看形状,合计不去对顶部大卡。 */
export function HourProfile({
  agentId,
  from,
  to,
  refreshKey,
  model = null,
  modelColor = null,
}: HourProfileProps) {
  const [rows, setRows] = useState<HourRow[] | null>(null);
  const requestId = useRef(0);
  const filterKey = `${agentId ?? ""}|${from}|${to}|${model ?? ""}`;
  const filterRef = useRef(filterKey);

  useEffect(() => {
    const current = ++requestId.current;
    let active = true;
    const filterChanged = filterRef.current !== filterKey;
    filterRef.current = filterKey;
    if (filterChanged) setRows(null);
    api
      .getHourProfile(agentId, from, to, model)
      .then((data) => {
        if (active && requestId.current === current) setRows(data);
      })
      .catch((err) => {
        console.error("[HourProfile] getHourProfile 失败", err);
        if (active && requestId.current === current) setRows([]);
      });
    return () => {
      active = false;
    };
  }, [agentId, from, to, refreshKey, model, filterKey]);

  const empty = rows != null && rows.every((r) => r.totalTokens === 0);
  const color = modelColor ?? "hsl(var(--primary))";

  return (
    <section
      data-theme-part="card chart-card"
      className="rounded-xl border border-border bg-card p-4 transition-all duration-300 hover:border-primary/60 hover:shadow-sm"
    >
      <div data-theme-part="card-title" className="flex items-center gap-1.5 text-sm font-semibold">
        <ClockIcon className="h-4 w-4 text-primary" />
        <span>使用时段</span>
        <span className="ml-1 min-w-0 text-xs font-normal text-muted-foreground">
          各天的同一小时相加,看习惯落在什么时候。按小时记录,近期可能不完整,只看形状
          {model ? (
            <>
              {" · "}
              <span className="inline-block max-w-[14rem] truncate align-bottom" title={model}>
                {model}
              </span>
            </>
          ) : null}
        </span>
      </div>

      {rows === null ? (
        <Skeleton className="mt-3 h-44" />
      ) : empty ? (
        <div className="mt-3">
          <EmptyState message="所选范围内暂无小时数据" />
        </div>
      ) : (
        <div data-theme-part="chart" className="mt-3 h-44 w-full">
          <ResponsiveContainer width="100%" height="100%">
            <BarChart data={rows} margin={{ top: 10, right: 8, left: 0, bottom: 0 }}>
              <CartesianGrid
                strokeDasharray="3 3"
                vertical={false}
                stroke="hsl(var(--border))"
                strokeOpacity={0.4}
              />
              <XAxis
                dataKey="hour"
                axisLine={false}
                tickLine={false}
                interval={2}
                dy={8}
                tick={{ fill: "hsl(var(--muted-foreground))", fontSize: 12 }}
                tickFormatter={(v: number) => String(v)}
              />
              <YAxis
                axisLine={false}
                tickLine={false}
                width={44}
                tick={{ fill: "hsl(var(--muted-foreground))", fontSize: 12 }}
                tickFormatter={(v: number) => fmtTokens(Number(v))}
              />
              <Tooltip
                content={<HourTooltip />}
                cursor={{ fill: "hsl(var(--muted))", opacity: 0.35 }}
                wrapperStyle={{ zIndex: 50, pointerEvents: "none" }}
              />
              <Bar dataKey="totalTokens" fill={color} maxBarSize={22} radius={[3, 3, 0, 0]} />
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}
    </section>
  );
}
