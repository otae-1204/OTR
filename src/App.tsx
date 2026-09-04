import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AGENT_LABELS,
  agentColor,
  api,
  type AgentStatus,
  type RangeSummary,
  type Totals,
} from "./api/bindings";
import { useUsageData } from "./hooks/useUsageData";
import { compareVersions, fetchLatestVersion } from "./lib/remote";
import { getVersion } from "@tauri-apps/api/app";
import {
  computeRange,
  PRESET_LABELS,
  rangeSubtitle,
  rangeTitle,
  todayStr,
  type RangePreset,
} from "./lib/range";
import { AgentCard } from "./components/AgentCard";
import { ModelPie } from "./components/ModelPie";
import { SessionTable } from "./components/SessionTable";
import { Settings } from "./components/Settings";
import { EmptyState, Skeleton } from "./components/Skeleton";
import { StatCard } from "./components/StatCard";
import { TrendChart } from "./components/TrendChart";
import {
  ActivityIcon,
  CalendarIcon,
  RefreshIcon,
  SettingsIcon,
} from "./components/icons";

type View = "dashboard" | "settings";

const THEME_KEY = "token-show-theme";
const SPIN_DURATION_MS = 1000;

const TOOLBAR_BTN =
  "flex h-8 w-8 items-center justify-center rounded-lg transition-colors hover:bg-black/5 dark:hover:bg-white/5";
const TOOLBAR_BTN_IDLE = "text-muted-foreground";
const TOOLBAR_BTN_ACTIVE = "bg-background shadow-sm text-foreground";

const PRESETS: RangePreset[] = ["today", "7d", "30d", "month", "all", "custom"];

const CHIP =
  "flex h-7 items-center gap-1.5 rounded-lg px-2.5 text-xs font-medium transition-colors";

export default function App() {
  const [view, setView] = useState<View>("dashboard");
  const [spinning, setSpinning] = useState(false);
  const spinTimerRef = useRef<number | null>(null);
  const { summary, agents, settings, loading, refresh } = useUsageData();

  // 筛选状态:Agent 维度 + 日期范围
  const [agentId, setAgentId] = useState<string | null>(null);
  const [preset, setPreset] = useState<RangePreset>("30d");
  const [customFrom, setCustomFrom] = useState(() =>
    new Date(Date.now() - 29 * 86400_000).toISOString().slice(0, 10),
  );
  const [customTo, setCustomTo] = useState(todayStr);
  const range = useMemo(
    () => computeRange(preset, customFrom, customTo),
    [preset, customFrom, customTo],
  );

  // 范围统计(Hero + 模型占比共用)
  const [rangeSummary, setRangeSummary] = useState<RangeSummary | null>(null);
  const [rangeLoading, setRangeLoading] = useState(false);
  // 竞态防护:只接受与当前筛选一致的响应(快速切换时旧响应可能更晚返回)
  const rangeReqRef = useRef("");

  // 恢复主题(index.html 默认 dark,本地记忆可切亮色)
  useEffect(() => {
    document.documentElement.classList.toggle(
      "dark",
      localStorage.getItem(THEME_KEY) !== "light",
    );
  }, []);

  useEffect(() => {
    return () => {
      if (spinTimerRef.current != null) {
        window.clearTimeout(spinTimerRef.current);
      }
    };
  }, []);

  const handleRefresh = useCallback(async () => {
    setSpinning(true);
    if (spinTimerRef.current != null) {
      window.clearTimeout(spinTimerRef.current);
    }
    spinTimerRef.current = window.setTimeout(() => {
      setSpinning(false);
      spinTimerRef.current = null;
    }, SPIN_DURATION_MS);
    try {
      await api.rescan(false);
    } catch (err) {
      console.error("[App] rescan 失败", err);
    }
    await refresh();
  }, [refresh]);

  /** summary 变化(刷新/事件/轮询)或启停 Agent 时驱动子组件重新拉取明细 */
  const enabledKey = (settings?.enabledAgents ?? []).slice().sort().join(",");
  const refreshKey = `${summary?.generatedAt ?? 0}:${enabledKey}`;

  useEffect(() => {
    if (view !== "dashboard") return;
    const reqKey = `${agentId ?? ""}|${range.from}|${range.to}|${enabledKey}`;
    rangeReqRef.current = reqKey;
    setRangeLoading(true);
    let active = true;
    api
      .getRangeSummary(agentId, range.from, range.to)
      .then((r) => {
        // 过期响应(筛选已再次变化)直接丢弃
        if (active && rangeReqRef.current === reqKey) {
          setRangeSummary(r);
          setRangeLoading(false);
        }
      })
      .catch((err) => {
        console.error("[App] getRangeSummary 失败", err);
        if (active && rangeReqRef.current === reqKey) {
          setRangeLoading(false);
        }
      });
    return () => {
      active = false;
    };
  }, [view, agentId, range.from, range.to, refreshKey]);

  /** 合并 listAgents 与 byAgentToday;设置里停用的不进主页 */
  const agentCards = useMemo(() => {
    const enabledIds = new Set(
      settings?.enabledAgents ??
        agents.filter((a) => a.enabled).map((a) => a.id),
    );
    const map = new Map<string, AgentStatus>();
    for (const a of agents) map.set(a.id, a);
    for (const id of Object.keys(AGENT_LABELS)) {
      if (!map.has(id)) {
        map.set(id, {
          id,
          displayName: AGENT_LABELS[id],
          detected: false,
          enabled: enabledIds.has(id),
          todayTokens: 0,
          todayCost: 0,
          totalTokens: 0,
        });
      }
    }
    const todayByAgent = new Map<string, Totals>(
      (summary?.byAgentToday ?? []).map((s) => [s.agent, s.totals] as const),
    );
    for (const [id, totals] of todayByAgent) {
      if (!map.has(id) && enabledIds.has(id)) {
        map.set(id, {
          id,
          displayName: id,
          detected: true,
          enabled: true,
          todayTokens: totals.totalTokens,
          todayCost: totals.cost,
          totalTokens: totals.totalTokens,
        });
      }
    }
    return [...map.values()]
      .filter((status) => enabledIds.has(status.id))
      .map((status) => ({
        status,
        today: todayByAgent.get(status.id),
      }));
  }, [agents, summary, settings]);

  useEffect(() => {
    if (agentId && !agentCards.some((c) => c.status.id === agentId)) {
      setAgentId(null);
    }
  }, [agentId, agentCards]);

  const agentName = agentId
    ? (agentCards.find((c) => c.status.id === agentId)?.status.displayName ??
      agentId)
    : null;
  const statTitle = agentName
    ? `${rangeTitle(preset, range)} · ${agentName}`
    : rangeTitle(preset, range);
  const sessionRange: { from: string | null; to: string | null } = range.all
    ? { from: null, to: null }
    : { from: range.from, to: range.to };
  /** 卡片跟随筛选:范围内各 Agent 的 tokens 与合计 */
  const rangeByAgent = useMemo(() => {
    const m = new Map<string, number>();
    let all = 0;
    for (const s of rangeSummary?.byAgent ?? []) {
      m.set(s.agent, s.totals.totalTokens);
      all += s.totals.totalTokens;
    }
    return { m, all };
  }, [rangeSummary]);
  const rangeLabel = preset === "custom" ? "该范围" : PRESET_LABELS[preset];
  const currency = settings?.currency ?? "CNY";
  const exchangeRate = settings?.exchangeRate ?? 7.2;

  // 版本检测:启动时查一次 GitHub Releases(失败静默)
  const [appVersion, setAppVersion] = useState("");
  const [updateLatest, setUpdateLatest] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    getVersion()
      .then((v) => {
        if (active) setAppVersion(v);
        return v;
      })
      .catch(() => "");
    fetchLatestVersion().then((latest) => {
      if (!active || !latest) return;
      getVersion()
        .then((cur) => {
          if (active && compareVersions(latest, cur) > 0) setUpdateLatest(latest);
        })
        .catch(() => undefined);
    });
    return () => {
      active = false;
    };
  }, []);

  return (
    <div className="min-h-screen bg-background text-foreground">
      <header
        data-tauri-drag-region
        className="fixed top-0 z-50 h-16 w-full border-b border-border/50 bg-background/80 backdrop-blur-md"
      >
        <div
          data-tauri-drag-region
          className="flex h-full items-center justify-between px-6"
        >
          <div data-tauri-drag-region className="flex items-center gap-2.5">
            <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-primary/10">
              <ActivityIcon className="h-4 w-4 text-primary" />
            </div>
            <div>
              <h1 className="text-sm font-semibold leading-tight">OTR</h1>
              <p className="text-xs leading-tight text-muted-foreground">
                Otae's Token Radar
              </p>
            </div>
          </div>

          <div className="flex items-center gap-1 rounded-xl bg-muted p-1">
            <button
              type="button"
              title="刷新数据"
              onClick={handleRefresh}
              className={`${TOOLBAR_BTN} ${TOOLBAR_BTN_IDLE}`}
            >
              <RefreshIcon
                className={`h-4 w-4 ${spinning ? "animate-spin" : ""}`}
              />
            </button>
            <button
              type="button"
              title={view === "settings" ? "返回仪表盘" : "设置"}
              onClick={() =>
                setView((v) => (v === "settings" ? "dashboard" : "settings"))
              }
              className={`relative ${TOOLBAR_BTN} ${
                view === "settings" ? TOOLBAR_BTN_ACTIVE : TOOLBAR_BTN_IDLE
              }`}
            >
              <SettingsIcon className="h-4 w-4" />
              {updateLatest ? (
                <span
                  className="absolute -right-0.5 -top-0.5 h-2 w-2 rounded-full bg-orange-500 ring-2 ring-background"
                  title={`发现新版本 v${updateLatest}`}
                />
              ) : null}
            </button>
          </div>
        </div>
      </header>

      <main className="min-h-screen">
        {view === "dashboard" ? (
          <div
            key="dashboard"
            className="mx-auto max-w-6xl animate-fade-in space-y-4 px-6 pb-10 pt-20"
          >
            {/* 筛选栏:Agent + 日期范围 */}
            <div className="sticky top-16 z-40 -mx-6 space-y-2 border-b border-border/40 bg-background/90 px-6 pb-3 pt-2 backdrop-blur-md">
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-xs text-muted-foreground">Agent</span>
                <button
                  type="button"
                  onClick={() => setAgentId(null)}
                  className={`${CHIP} border ${
                    agentId === null
                      ? "border-primary/50 bg-primary/15 text-foreground"
                      : "border-transparent text-muted-foreground hover:bg-black/5 dark:hover:bg-white/5"
                  }`}
                >
                  全部
                </button>
                {agentCards.map(({ status }) => (
                  <button
                    key={status.id}
                    type="button"
                    onClick={() =>
                      setAgentId(agentId === status.id ? null : status.id)
                    }
                    className={`${CHIP} border ${
                      agentId === status.id
                        ? "border-primary/50 bg-primary/15 text-foreground"
                        : "border-transparent text-muted-foreground hover:bg-black/5 dark:hover:bg-white/5"
                    } ${status.detected ? "" : "opacity-50"}`}
                  >
                    <span
                      className="h-2 w-2 rounded-full"
                      style={{ backgroundColor: agentColor(status.id) }}
                    />
                    {status.displayName || status.id}
                  </button>
                ))}
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <span className="flex items-center gap-1 text-xs text-muted-foreground">
                  <CalendarIcon className="h-3.5 w-3.5" />
                  范围
                </span>
                {PRESETS.map((p) => (
                  <button
                    key={p}
                    type="button"
                    onClick={() => setPreset(p)}
                    className={`${CHIP} ${
                      preset === p
                        ? "bg-background shadow-sm text-foreground ring-1 ring-border"
                        : "text-muted-foreground hover:bg-black/5 dark:hover:bg-white/5"
                    } bg-muted`}
                  >
                    {PRESET_LABELS[p]}
                  </button>
                ))}
                {preset === "custom" ? (
                  <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <input
                      type="date"
                      value={customFrom}
                      max={customTo}
                      onChange={(e) => setCustomFrom(e.target.value)}
                      className="h-7 rounded-lg border border-border bg-background px-2 text-xs outline-none focus:border-primary"
                    />
                    ~
                    <input
                      type="date"
                      value={customTo}
                      min={customFrom}
                      onChange={(e) => setCustomTo(e.target.value)}
                      className="h-7 rounded-lg border border-border bg-background px-2 text-xs outline-none focus:border-primary"
                    />
                  </span>
                ) : null}
              </div>
            </div>

            {loading && !summary ? (
              <div className="space-y-4">
                <Skeleton className="h-40" />
                <div className="grid gap-3 md:grid-cols-2 lg:grid-cols-4">
                  {[0, 1, 2, 3].map((i) => (
                    <Skeleton key={i} />
                  ))}
                </div>
                <div className="grid gap-3 lg:grid-cols-2">
                  <Skeleton className="h-80" />
                  <Skeleton className="h-80" />
                </div>
                <Skeleton className="h-64" />
              </div>
            ) : summary ? (
              <>
                <StatCard
                  summary={rangeSummary}
                  title={statTitle}
                  subtitle={rangeSubtitle(preset, range)}
                  loading={rangeLoading}
                  currency={currency}
                  rate={exchangeRate}
                  agentAllTimeTokens={
                    agentId
                      ? (agentCards.find((c) => c.status.id === agentId)
                          ?.status.totalTokens ?? null)
                      : null
                  }
                />

                <div className="grid gap-3 md:grid-cols-2 lg:grid-cols-4">
                  {agentCards.map(({ status }) => (
                    <AgentCard
                      key={status.id}
                      status={status}
                      rangeTokens={rangeByAgent.m.get(status.id) ?? 0}
                      rangeAllTokens={rangeByAgent.all}
                      rangeLabel={rangeLabel}
                      selected={agentId === status.id}
                      onSelect={setAgentId}
                    />
                  ))}
                </div>

                <div className="grid items-start gap-3 lg:grid-cols-2">
                  <TrendChart
                    agentId={agentId}
                    from={range.from}
                    to={range.to}
                    all={range.all}
                    refreshKey={refreshKey}
                  />
                  <ModelPie
                    summary={rangeSummary}
                    rangeLabel={rangeTitle(preset, range)}
                    currency={currency}
                    rate={exchangeRate}
                  />
                </div>

                <SessionTable
                  agentId={agentId}
                  from={sessionRange.from}
                  to={sessionRange.to}
                  refreshKey={refreshKey}
                  rangeLabel={rangeTitle(preset, range)}
                  currency={currency}
                  rate={exchangeRate}
                />
              </>
            ) : (
              <div className="pt-6">
                <EmptyState message="数据加载失败,请确认后端服务正在运行后点击顶栏刷新重试" />
              </div>
            )}
          </div>
        ) : (
          <div
            key="settings"
            className="mx-auto max-w-3xl animate-fade-in space-y-4 px-6 pb-10 pt-20"
          >
            <Settings
              agents={agents}
              onDataChanged={refresh}
              appVersion={appVersion}
              updateLatest={updateLatest}
            />
          </div>
        )}
      </main>
    </div>
  );
}
