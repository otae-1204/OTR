import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AGENT_LABELS,
  api,
  type AgentStatus,
  type RangeSummary,
  type Totals,
} from "./api/bindings";
import { useUsageData } from "./hooks/useUsageData";
import { useTheme } from "./theme/ThemeProvider";
import { compareVersions, fetchLatestVersion } from "./lib/remote";
import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  computeRange,
  previousRange,
  PRESET_LABELS,
  rangeSubtitle,
  rangeTitle,
  todayStr,
  type RangePreset,
} from "./lib/range";
import { AgentCard } from "./components/AgentCard";
import { AgentIconOverrides } from "./components/AgentIcon";
import { ModelPie } from "./components/ModelPie";
import { Limits } from "./components/Limits";
import { SessionTable } from "./components/SessionTable";
import { Settings } from "./components/Settings";
import { EmptyState, Skeleton } from "./components/Skeleton";
import { SelectMenu } from "./components/SelectMenu";
import { StatCard } from "./components/StatCard";
import { ThemeMascot } from "./components/ThemeMascot";
import { isMacPlatform, WindowControls } from "./components/WindowControls";
import { TrendChart } from "./components/TrendChart";
import { HourProfile } from "./components/HourProfile";
import {
  ActivityIcon,
  CalendarIcon,
  GaugeIcon,
  RefreshIcon,
  SettingsIcon,
} from "./components/icons";

type View = "dashboard" | "limits" | "settings";

const SPIN_DURATION_MS = 1000;

const PRESETS: RangePreset[] = ["today", "7d", "30d", "month", "all", "custom"];

const CHIP =
  "flex h-7 items-center gap-1.5 rounded-lg px-2.5 text-xs font-medium transition-[color,background-color,border-color,box-shadow] duration-200";

export default function App() {
  const [view, setView] = useState<View>("dashboard");
  const [spinning, setSpinning] = useState(false);
  const [limitsEpoch, setLimitsEpoch] = useState(0);
  const spinTimerRef = useRef<number | null>(null);
  const { summary, agents, settings, loading, refresh } = useUsageData();
  const { agentColor, chartPalette } = useTheme();
  const [agentIconUrls, setAgentIconUrls] = useState<Record<string, string>>({});
  const agentIconKey = JSON.stringify(settings?.agentIcons ?? {});

  // 筛选状态:Agent 维度 + 日期范围
  const [agentId, setAgentId] = useState<string | null>(null);
  const [preset, setPreset] = useState<RangePreset>("30d");
  /** 扇区点选,含「其他」。只有真实模型名会驱动趋势图和时段图 */
  const [piePick, setPiePick] = useState<string | null>(null);
  const [customFrom, setCustomFrom] = useState(() =>
    new Date(Date.now() - 29 * 86400_000).toISOString().slice(0, 10),
  );
  const [customTo, setCustomTo] = useState(todayStr);
  const range = useMemo(
    () => computeRange(preset, customFrom, customTo),
    [preset, customFrom, customTo],
  );
  const prevWindow = useMemo(
    () => previousRange(preset, range),
    [preset, range],
  );

  // 范围统计(Hero + 模型占比共用)
  const [rangeSummary, setRangeSummary] = useState<RangeSummary | null>(null);
  const [prevSummary, setPrevSummary] = useState<RangeSummary | null>(null);
  const [rangeLoading, setRangeLoading] = useState(false);
  // 竞态防护:只接受与当前筛选一致的响应(快速切换时旧响应可能更晚返回)
  const rangeReqRef = useRef("");
  const prevReqRef = useRef("");

  useEffect(() => {
    let active = true;
    api
      .agentIconData()
      .then((urls) => {
        if (active) setAgentIconUrls(urls);
      })
      .catch(() => {
        if (active) setAgentIconUrls({});
      });
    return () => {
      active = false;
    };
  }, [agentIconKey]);

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
    const started = Date.now();
    try {
      // 用量重扫不包含额度。套餐窗口要单独拉,额度页靠 limitsEpoch 读缓存。
      await Promise.all([
        api.rescan(false).catch((err) => {
          console.error("[App] rescan 失败", err);
        }),
        api
          .refreshLimits()
          .then(() => setLimitsEpoch((n) => n + 1))
          .catch((err) => {
            console.error("[App] 额度刷新失败", err);
          }),
        refresh(),
      ]);
    } finally {
      const remain = Math.max(0, SPIN_DURATION_MS - (Date.now() - started));
      spinTimerRef.current = window.setTimeout(() => {
        setSpinning(false);
        spinTimerRef.current = null;
      }, remain);
    }
  }, [refresh]);

  /**
   * 数据版本 + 启停 Agent 决定子组件何时重拉明细。
   * 以前用 summary.generatedAt,而它是 now_ms() —— 每 30 秒的轮询都会让它变,
   * 于是 get_range_summary + get_daily + get_sessions 每轮白跑一遍。
   * dataVersion 只在后端真的写入用量时递增,空闲时完全不动。
   */
  const enabledKey = (settings?.enabledAgents ?? []).slice().sort().join(",");
  const refreshKey = `${summary?.dataVersion ?? 0}:${enabledKey}`;

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

  useEffect(() => {
    if (view !== "dashboard") return;
    if (!prevWindow) {
      prevReqRef.current = "";
      setPrevSummary(null);
      return;
    }
    const reqKey = `${agentId ?? ""}|${prevWindow.from}|${prevWindow.to}|${enabledKey}`;
    prevReqRef.current = reqKey;
    let active = true;
    api
      .getRangeSummary(agentId, prevWindow.from, prevWindow.to)
      .then((r) => {
        if (active && prevReqRef.current === reqKey) setPrevSummary(r);
      })
      .catch((err) => {
        console.error("[App] 上一周期 getRangeSummary 失败", err);
        if (active && prevReqRef.current === reqKey) setPrevSummary(null);
      });
    return () => {
      active = false;
    };
  }, [view, agentId, prevWindow, enabledKey, refreshKey]);

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
          totalTokens: 0,
          notice: null,
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
          totalTokens: totals.totalTokens,
          notice: null,
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
  const previous =
    prevWindow &&
    rangeSummary &&
    prevSummary &&
    rangeSummary.from === range.from &&
    rangeSummary.to === range.to &&
    (rangeSummary.agent ?? null) === agentId &&
    prevSummary.from === prevWindow.from &&
    prevSummary.to === prevWindow.to &&
    (prevSummary.agent ?? null) === agentId
      ? {
          label: prevWindow.label,
          from: prevWindow.from,
          to: prevWindow.to,
          totals: prevSummary.totals,
        }
      : null;
  const chartModel = piePick && piePick !== "其他" ? piePick : null;
  const modelColor = useMemo(() => {
    if (!chartModel || !rangeSummary) return null;
    const index = rangeSummary.byModel.findIndex((m) => m.model === chartModel);
    if (index < 0 || index >= 8 || chartPalette.length === 0) return null;
    return chartPalette[index % chartPalette.length];
  }, [chartModel, rangeSummary, chartPalette]);

  useEffect(() => {
    if (!piePick || !rangeSummary) return;
    if (rangeSummary.from !== range.from || rangeSummary.to !== range.to) return;
    if ((rangeSummary.agent ?? null) !== agentId) return;
    if (piePick === "其他") {
      if (rangeSummary.byModel.length <= 8) setPiePick(null);
      return;
    }
    const visible = rangeSummary.byModel
      .slice(0, 8)
      .some((m) => m.model === piePick);
    if (!visible) setPiePick(null);
  }, [piePick, rangeSummary, range.from, range.to, agentId]);

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
    <AgentIconOverrides.Provider value={agentIconUrls}>
    <div data-theme-part="app" className="min-h-screen bg-background text-foreground">
      <ThemeMascot />
      <header
        data-theme-part="header"
        className="fixed top-0 z-50 flex h-16 w-full border-b border-border/50 bg-background text-foreground"
      >
        <div
          className={`flex h-full min-w-0 flex-1 items-center justify-between pr-6 ${
            isMacPlatform() ? "pl-[5.25rem]" : "pl-6"
          }`}
          onMouseDown={(e) => {
            if (e.buttons !== 1) return;
            const target = e.target;
            if (!(target instanceof Element)) return;
            if (target.closest("button, a, input, textarea, select")) return;
            e.preventDefault();
            void getCurrentWindow().startDragging();
          }}
          onDoubleClick={(e) => {
            const target = e.target;
            if (target instanceof Element && target.closest("button, a, input, textarea, select")) {
              return;
            }
            getCurrentWindow().toggleMaximize();
          }}
        >
          <div className="flex items-center gap-2.5">
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

          <div className="flex items-center gap-2.5">
            <button
              type="button"
              title="刷新数据"
              aria-label="刷新数据"
              onClick={handleRefresh}
              data-theme-part="button"
              className="flex h-8 w-8 items-center justify-center rounded-lg border border-border/60 bg-background/50 text-muted-foreground transition-colors hover:bg-overlay/5 hover:text-foreground"
            >
              <RefreshIcon
                className={`h-4 w-4 ${spinning ? "animate-spin" : ""}`}
              />
            </button>
            <div className="h-4 w-px bg-border/60" />
            <div data-theme-part="segmented" className="flex items-center gap-1 rounded-xl bg-muted p-1">
              <button
                type="button"
                onClick={() => setView("dashboard")}
                data-theme-part="segmented-button"
                data-theme-state={view === "dashboard" ? "selected" : undefined}
                className={`flex h-7 items-center gap-1.5 rounded-lg px-2.5 text-xs font-medium transition-[color,background-color,box-shadow] duration-200 ${
                  view === "dashboard"
                    ? "bg-background shadow-sm text-foreground"
                    : "text-muted-foreground hover:bg-overlay/5 hover:text-foreground"
                }`}
              >
                <ActivityIcon className="h-3.5 w-3.5" />
                概览
              </button>
              <button
                type="button"
                onClick={() => setView("limits")}
                data-theme-part="segmented-button"
                data-theme-state={view === "limits" ? "selected" : undefined}
                className={`flex h-7 items-center gap-1.5 rounded-lg px-2.5 text-xs font-medium transition-[color,background-color,box-shadow] duration-200 ${
                  view === "limits"
                    ? "bg-background shadow-sm text-foreground"
                    : "text-muted-foreground hover:bg-overlay/5 hover:text-foreground"
                }`}
              >
                <GaugeIcon className="h-3.5 w-3.5" />
                额度
              </button>
              <button
                type="button"
                onClick={() => setView("settings")}
                data-theme-part="segmented-button"
                data-theme-state={view === "settings" ? "selected" : undefined}
                className={`relative flex h-7 items-center gap-1.5 rounded-lg px-2.5 text-xs font-medium transition-[color,background-color,box-shadow] duration-200 ${
                  view === "settings"
                    ? "bg-background shadow-sm text-foreground"
                    : "text-muted-foreground hover:bg-overlay/5 hover:text-foreground"
                }`}
              >
                <SettingsIcon className="h-3.5 w-3.5" />
                设置
                {updateLatest ? (
                  <span
                    className="absolute -right-0.5 -top-0.5 h-2 w-2 rounded-full bg-notice ring-2 ring-background"
                    title={`发现新版本 v${updateLatest}`}
                  />
                ) : null}
              </button>
            </div>
          </div>
        </div>
        {isMacPlatform() ? null : <WindowControls />}
      </header>

      <main data-theme-part="main" className="relative z-10 min-h-screen">
        {view === "dashboard" ? (
          <div
            key="dashboard"
            className="mx-auto max-w-6xl animate-fade-in space-y-4 px-6 pb-10 pt-20"
          >
            {/* 筛选栏:Agent 下拉 + 日期范围单行小胶囊 */}
            <div className="sticky top-16 z-40">
              <div
                data-theme-part="filter-bar"
                className="flex w-fit max-w-full flex-wrap items-center gap-1.5 rounded-xl border border-border/50 bg-card/40 px-2.5 py-1.5 backdrop-blur-md"
              >
                <SelectMenu
                  value={agentId ?? ""}
                  onChange={(v) => setAgentId(v || null)}
                  ariaLabel="筛选 Agent"
                  className="h-7 min-w-[124px] max-w-[180px]"
                  options={[
                    { value: "", label: "全部 Agent" },
                    ...agentCards.map(({ status }) => ({
                      value: status.id,
                      label: status.displayName || status.id,
                      dotColor: agentColor(status.id),
                      muted: !status.detected,
                    })),
                  ]}
                />
                <span className="mx-0.5 h-4 w-px shrink-0 bg-border/60" />
                <span className="flex items-center text-muted-foreground" title="日期范围">
                  <CalendarIcon className="h-3.5 w-3.5" />
                </span>
                {PRESETS.map((p) => (
                  <button
                    key={p}
                    type="button"
                    onClick={() => setPreset(p)}
                    data-theme-part="chip"
                    data-theme-state={preset === p ? "selected" : undefined}
                    className={`${CHIP} border ${
                      preset === p
                        ? "border-border bg-background shadow-sm text-foreground"
                        : "border-transparent text-muted-foreground hover:bg-overlay/5"
                    }`}
                  >
                    {PRESET_LABELS[p]}
                  </button>
                ))}
                {preset === "custom" ? (
                  <span className="flex items-center gap-1.5 text-xs text-muted-foreground animate-fade-soft">
                    <input
                      type="date"
                      value={customFrom}
                      max={customTo}
                      onChange={(e) => setCustomFrom(e.target.value)}
                      data-theme-part="input"
                      className="h-7 rounded-lg border border-border bg-background px-2 text-xs outline-none focus:border-primary"
                    />
                    ~
                    <input
                      type="date"
                      value={customTo}
                      min={customFrom}
                      onChange={(e) => setCustomTo(e.target.value)}
                      data-theme-part="input"
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
                  previous={previous}
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

                <div className="grid items-stretch gap-3 lg:grid-cols-2">
                  <TrendChart
                    agentId={agentId}
                    from={range.from}
                    to={range.to}
                    all={range.all}
                    refreshKey={refreshKey}
                    previous={prevWindow}
                    model={chartModel}
                    modelColor={modelColor}
                  />
                  <ModelPie
                    summary={rangeSummary}
                    rangeLabel={rangeTitle(preset, range)}
                    currency={currency}
                    rate={exchangeRate}
                    selected={piePick}
                    onSelect={setPiePick}
                  />
                </div>

                <HourProfile
                  agentId={agentId}
                  from={range.from}
                  to={range.to}
                  refreshKey={refreshKey}
                  model={chartModel}
                  modelColor={modelColor}
                />

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
        ) : view === "limits" ? (
          <div
            key="limits"
            className="mx-auto max-w-6xl animate-fade-in space-y-4 px-6 pb-10 pt-20"
          >
            <Limits refreshEpoch={limitsEpoch} />
          </div>
        ) : (
          <div
            key="settings"
            className="mx-auto max-w-5xl animate-fade-in space-y-4 px-6 pb-10 pt-20"
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
    </AgentIconOverrides.Provider>
  );
}
