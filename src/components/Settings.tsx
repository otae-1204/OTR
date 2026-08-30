import { useEffect, useState, type ReactNode } from "react";
import {
  AGENT_COLORS,
  AGENT_LABELS,
  agentColor,
  api,
  type AgentStatus,
  type CustomAgentConfig,
  type PriceEntry,
  type Settings as AppSettings,
} from "../api/bindings";
import { fmtTokens } from "../lib/format";
import {
  CoinsIcon,
  DatabaseIcon,
  MoonIcon,
  PlusIcon,
  RefreshIcon,
  SunIcon,
  TrashIcon,
} from "./icons";
import { AgentIcon, usesImageIcon } from "./AgentIcon";
import { fetchLatestVersion, fetchUsdCnyRate } from "../lib/remote";

const KNOWN_AGENTS = [
  "dsh",
  "claude-code",
  "codex",
  "zcode",
  "opencode",
  "pi",
];
const THEME_KEY = "token-show-theme";

const KIND_OPTIONS: { value: string; label: string; hint: string }[] = [
  {
    value: "claude-code",
    label: "Claude Code 布局",
    hint: "projects/<路径编码>/<会话>.jsonl(CC 系 fork 通用,如 CodeBuddy)",
  },
  {
    value: "codex",
    label: "Codex 布局",
    hint: "sessions/YYYY/MM/DD/rollout-*.jsonl",
  },
  {
    value: "zcode",
    label: "ZCode 布局",
    hint: "rollout/*.jsonl(每次模型调用一行 usage)",
  },
];

function kindLabel(kind: string): string {
  return KIND_OPTIONS.find((k) => k.value === kind)?.label ?? kind;
}

function Badge({
  tone,
  children,
}: {
  tone: "good" | "muted";
  children: ReactNode;
}) {
  return (
    <span
      className={`rounded-md px-1.5 py-0.5 text-xs ${
        tone === "good"
          ? "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
          : "bg-muted text-muted-foreground"
      }`}
    >
      {children}
    </span>
  );
}

/** 自制开关:w-11 h-6 轨道,选中 bg-emerald-500,thumb 平移动画 */
function Toggle({
  checked,
  disabled,
  onChange,
  ariaLabel,
}: {
  checked: boolean;
  disabled?: boolean;
  onChange: () => void;
  ariaLabel: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={onChange}
      className={`relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 ${
        checked ? "bg-emerald-500" : "bg-muted-foreground/30"
      } disabled:cursor-not-allowed disabled:opacity-50`}
    >
      <span
        className={`inline-block h-5 w-5 rounded-full bg-white shadow transition-transform duration-200 ${
          checked ? "translate-x-5" : "translate-x-0.5"
        }`}
      />
    </button>
  );
}

function SectionCard({
  icon,
  title,
  description,
  children,
}: {
  icon: ReactNode;
  title: string;
  description: string;
  children: ReactNode;
}) {
  return (
    <section className="overflow-hidden rounded-xl border border-border bg-card transition-all duration-300 hover:border-primary/60 hover:shadow-sm">
      <div className="border-b border-border/40 px-4 py-3">
        <h3 className="flex items-center gap-1.5 text-sm font-semibold">
          {icon}
          {title}
        </h3>
        <p className="mt-0.5 text-xs text-muted-foreground">{description}</p>
      </div>
      {children}
    </section>
  );
}

function slugify(name: string, existing: string[]): string {
  const base =
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "agent";
  let id = `custom-${base}`;
  let i = 2;
  while (existing.includes(id)) {
    id = `custom-${base}-${i}`;
    i++;
  }
  return id;
}

interface SettingsProps {
  agents: AgentStatus[];
  /** 设置变更 / 手动扫描后通知父级刷新数据 */
  onDataChanged: () => void;
  appVersion: string;
  /** GitHub 检测到的新版本号,null 表示无更新 */
  updateLatest: string | null;
}

export function Settings({
  agents,
  onDataChanged,
  appVersion,
  updateLatest,
}: SettingsProps) {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [busyAction, setBusyAction] = useState<"none" | "refresh" | "rescan">(
    "none",
  );
  const [theme, setTheme] = useState<"dark" | "light">(() =>
    document.documentElement.classList.contains("dark") ? "dark" : "light",
  );
  // 自定义 Agent 表单
  const [updateState, setUpdateState] = useState<
    "idle" | "checking" | "latest" | "available" | "error"
  >("idle");
  const [updateMsg, setUpdateMsg] = useState("");
  const [fxState, setFxState] = useState<"idle" | "loading" | "ok" | "error">("idle");
  const [fxMsg, setFxMsg] = useState("");
  const [cName, setCName] = useState("");
  const [cKind, setCKind] = useState<CustomAgentConfig["kind"]>("claude-code");
  const [cDir, setCDir] = useState("");
  // 成本定价
  const [models, setModels] = useState<string[]>([]);
  const [fetchState, setFetchState] = useState<"idle" | "loading" | "ok" | "error">("idle");
  const [fetchMsg, setFetchMsg] = useState("");

  useEffect(() => {
    let active = true;
    api
      .getSettings()
      .then((s) => {
        if (active) setSettings(s);
      })
      .catch((err) => console.error("[Settings] getSettings 失败", err));
    api
      .listModels()
      .then((m) => {
        if (active) setModels(m);
      })
      .catch((err) => console.error("[Settings] listModels 失败", err));
    return () => {
      active = false;
    };
  }, []);

  const persist = async (next: AppSettings) => {
    setSettings(next);
    try {
      await api.saveSettings(next);
    } catch (err) {
      console.error("[Settings] saveSettings 失败", err);
    }
    onDataChanged();
  };

  const toggleAgent = (id: string) => {
    if (!settings) return;
    const enabled = new Set(settings.enabledAgents);
    if (enabled.has(id)) {
      enabled.delete(id);
    } else {
      enabled.add(id);
    }
    void persist({ ...settings, enabledAgents: [...enabled] });
  };

  const applyTheme = (mode: "dark" | "light") => {
    document.documentElement.classList.toggle("dark", mode === "dark");
    localStorage.setItem(THEME_KEY, mode);
    setTheme(mode);
    if (settings) {
      const next = { ...settings, theme: mode };
      setSettings(next);
      void api.saveSettings(next).catch(() => undefined);
    }
  };

  const checkUpdate = async () => {
    setUpdateState("checking");
    setUpdateMsg("正在从 GitHub Releases 获取最新版本…");
    try {
      const latest = await fetchLatestVersion();
      if (!latest) {
        setUpdateState("error");
        setUpdateMsg("获取失败:网络无法访问 GitHub,稍后再试");
        return;
      }
      if (updateLatest && updateLatest !== latest) {
        onDataChanged(); // 触发 App 头部橙点刷新
      }
      const cur = appVersion || "0.0.0";
      if (latest === cur) {
        setUpdateState("latest");
        setUpdateMsg(`已是最新版本 v${cur}`);
      } else if (latest > cur) {
        setUpdateState("available");
        setUpdateMsg(`发现新版本 v${latest}(当前 v${cur}),前往 Releases 页下载`);
      } else {
        setUpdateState("latest");
        setUpdateMsg(`本地 v${cur} 比 Releases 上的 v${latest} 还新(开发版?)`);
      }
    } catch (err) {
      setUpdateState("error");
      setUpdateMsg("获取失败,请检查网络");
    }
  };

  const fetchFxRate = async () => {
    if (!settings) return;
    setFxState("loading");
    setFxMsg("正在获取实时汇率…");
    try {
      const res = await fetchUsdCnyRate();
      if (!res) throw new Error("unavailable");
      const next = { ...settings, exchangeRate: res.rate };
      setSettings(next);
      await api.saveSettings(next);
      setFxState("ok");
      setFxMsg(`已更新为 ¥${res.rate.toFixed(4)}/$(来源:${res.source})`);
    } catch (err) {
      setFxState("error");
      setFxMsg("汇率获取失败,请检查网络后手动填写");
    }
  };

  const handleRefresh = async () => {
    setBusyAction("refresh");
    try {
      await api.rescan(false);
      onDataChanged();
    } catch (err) {
      console.error("[Settings] rescan 失败", err);
    } finally {
      setBusyAction("none");
    }
  };

  const handleFullRescan = async () => {
    if (
      !window.confirm(
        "全量重扫将清除缓存并重新解析所有本地记录,可能耗时较长。确定继续吗?",
      )
    ) {
      return;
    }
    setBusyAction("rescan");
    try {
      await api.rescan(true);
      onDataChanged();
    } catch (err) {
      console.error("[Settings] 全量重扫失败", err);
    } finally {
      setBusyAction("none");
    }
  };

  const addCustom = () => {
    if (!settings) return;
    const name = cName.trim();
    const dir = cDir.trim().replace(/[/\\]+$/, "");
    if (!name || !dir) {
      window.alert("请填写名称和数据目录");
      return;
    }
    if (
      settings.customAgents.some(
        (c) => c.dir.toLowerCase() === dir.toLowerCase(),
      )
    ) {
      window.alert("该目录已添加过");
      return;
    }
    const cfg: CustomAgentConfig = {
      id: slugify(
        name,
        settings.customAgents.map((c) => c.id),
      ),
      name,
      kind: cKind,
      dir,
    };
    setCName("");
    setCDir("");
    void persist({
      ...settings,
      customAgents: [...settings.customAgents, cfg],
      enabledAgents: [...settings.enabledAgents, cfg.id],
    });
  };

  const removeCustom = (id: string) => {
    if (!settings) return;
    const cfg = settings.customAgents.find((c) => c.id === id);
    if (!cfg) return;
    if (!window.confirm(`删除自定义 Agent「${cfg.name}」?其已统计的数据会保留。`)) {
      return;
    }
    void persist({
      ...settings,
      customAgents: settings.customAgents.filter((c) => c.id !== id),
      enabledAgents: settings.enabledAgents.filter((a) => a !== id),
    });
  };

  /** 更新某个模型的单价字段(输入/输出/缓存读/缓存写,$/M),仅改本地状态 */
  const updatePrice = (model: string, field: keyof PriceEntry, raw: string) => {
    if (!settings) return;
    const cur: PriceEntry = settings.pricing[model] ?? {
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
    };
    const v = parseFloat(raw);
    const next: PriceEntry = { ...cur, [field]: Number.isFinite(v) && v >= 0 ? v : 0 };
    setSettings({ ...settings, pricing: { ...settings.pricing, [model]: next } });
  };

  const persistNow = () => {
    if (!settings) return;
    void api.saveSettings(settings).catch(() => undefined);
  };

  const clearPrice = (model: string) => {
    if (!settings) return;
    const pricing = { ...settings.pricing };
    delete pricing[model];
    const next = { ...settings, pricing };
    setSettings(next);
    void api.saveSettings(next).catch(() => undefined);
  };

  const updateCurrency = (currency: string) => {
    if (!settings) return;
    const next = { ...settings, currency };
    setSettings(next);
    void api.saveSettings(next).catch(() => undefined);
  };

  const updateExchangeRate = (raw: string) => {
    if (!settings) return;
    const v = parseFloat(raw);
    const next = {
      ...settings,
      exchangeRate: Number.isFinite(v) && v > 0 ? v : settings.exchangeRate,
    };
    setSettings(next);
  };

  /** 参考 CC-Switch:从 models.dev 拉取全量目录,匹配本地出现过的模型写入定价 */
  const fetchModelsDev = async () => {
    if (!settings) return;
    setFetchState("loading");
    setFetchMsg("正在拉取 models.dev 目录…");
    try {
      const res = await fetch("https://models.dev/api.json");
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      // 同一个模型 id 会在几十个 provider 下重复出现,价格差异巨大(abacus $6/M vs openai 官方 $1.2/M)。
      // 按 CC-Switch 的思路优先取"模型家族的官方 provider":gpt→openai、claude→anthropic、deepseek→deepseek…
      const familyProvider = (model: string): string | null => {
        const m = model.toLowerCase();
        if (/^(gpt|o\d|chatgpt)/.test(m)) return "openai";
        if (/^claude/.test(m)) return "anthropic";
        if (/^deepseek/.test(m)) return "deepseek";
        if (/^(gemini|gemma)/.test(m)) return "google";
        if (/^qwen/.test(m)) return "qwen";
        if (/^kimi/.test(m)) return "moonshotai";
        if (/^glm/.test(m)) return "zai";
        if (/^grok/.test(m)) return "xai";
        if (/^minimax/.test(m)) return "minimax";
        return null;
      };
      const toEntry = (m: any): PriceEntry | null => {
        const c = m?.cost;
        if (!c || typeof c.input !== "number") return null;
        return {
          input: c.input ?? 0,
          output: c.output ?? 0,
          cacheRead: c.cache_read ?? 0,
          cacheWrite: c.cache_write ?? 0,
        };
      };
      // id(小写) → [{provider, entry}],同 id 多 provider 时按上面的优先级挑
      const catalog = new Map<string, { provider: string; entry: PriceEntry }[]>();
      for (const [pid, prov] of Object.entries<any>(data ?? {})) {
        for (const [id, m] of Object.entries<any>(prov?.models ?? {})) {
          const entry = toEntry(m);
          if (!entry) continue;
          const key = id.toLowerCase();
          const list = catalog.get(key) ?? [];
          list.push({ provider: pid, entry });
          catalog.set(key, list);
        }
      }
      const pick = (model: string): PriceEntry | null => {
        const key = model.toLowerCase();
        const candidates = [key, model.split("/").pop()?.toLowerCase() ?? ""].filter(Boolean);
        for (const c of candidates) {
          const list = catalog.get(c);
          if (!list || list.length === 0) continue;
          const fam = familyProvider(c);
          const chosen = (fam && list.find((x) => x.provider === fam)) || list[0];
          return chosen.entry;
        }
        return null;
      };
      const next = { ...settings.pricing };
      let matched = 0;
      for (const model of models) {
        const found = pick(model);
        if (found) {
          next[model] = found;
          matched++;
        }
      }
      await persist({ ...settings, pricing: next });
      setFetchState("ok");
      setFetchMsg(`已匹配 ${matched} / ${models.length} 个本地模型的官方定价`);
    } catch (err) {
      console.error("[Settings] models.dev 获取失败", err);
      setFetchState("error");
      setFetchMsg("获取失败,请检查网络后重试(也可手动填写)");
    }
  };

  /** 数据源区块的行:内置 + 自定义 */
  const rows: {
    id: string;
    label: string;
    kindBadge?: string;
    dir?: string;
  }[] = KNOWN_AGENTS.map((id) => ({
    id,
    label: AGENT_LABELS[id] ?? id,
  }));
  for (const c of settings?.customAgents ?? []) {
    rows.push({ id: c.id, label: c.name, kindBadge: kindLabel(c.kind), dir: c.dir });
  }

  return (
    <div className="space-y-4">
      <div>
        <h2 className="text-lg font-semibold">设置</h2>
        <p className="mt-0.5 text-xs text-muted-foreground">
          管理数据源、扫描与外观,改动即时保存
        </p>
      </div>

      <SectionCard
        icon={<DatabaseIcon className="h-4 w-4 text-primary" />}
        title="数据源"
        description="各 AI Coding Agent 的检测状态与统计开关"
      >
        <div>
          {rows.map((row) => {
            const status = agents.find((a) => a.id === row.id);
            const detected = status?.detected ?? false;
            const enabled = settings
              ? settings.enabledAgents.includes(row.id)
              : (status?.enabled ?? false);
            const color = agentColor(row.id);
            const label = row.label;
            return (
              <div
                key={row.id}
                className="flex items-center justify-between border-b border-border/40 px-4 py-3 last:border-0"
              >
                <div className="flex min-w-0 items-center gap-3">
                  <div
                    className={`flex h-9 w-9 shrink-0 items-center justify-center overflow-hidden rounded-lg border border-border/40`}
                    style={{ backgroundColor: `${color}1A`, color }}
                  >
                    <AgentIcon
                      id={row.id}
                      color={color}
                      className={
                        usesImageIcon(row.id) ? "h-full w-full" : "h-5 w-5"
                      }
                    />
                  </div>
                  <div className="min-w-0">
                    <div className="flex items-center gap-2 text-sm font-medium">
                      <span className="truncate">{label}</span>
                      {row.kindBadge ? (
                        <Badge tone="muted">{row.kindBadge}</Badge>
                      ) : null}
                    </div>
                    <div className="mt-1 flex flex-wrap items-center gap-1.5">
                      <Badge tone={detected ? "good" : "muted"}>
                        {detected ? "已检测到" : "未检测到"}
                      </Badge>
                      <Badge tone={enabled ? "good" : "muted"}>
                        {enabled ? "已启用" : "已停用"}
                      </Badge>
                      {status && status.totalTokens > 0 ? (
                        <span
                          className="text-xs text-muted-foreground"
                          title={`累计 ${status.totalTokens.toLocaleString()} tokens`}
                        >
                          累计 {fmtTokens(status.totalTokens)}
                        </span>
                      ) : null}
                    </div>
                    {row.dir ? (
                      <div
                        className="mt-1 max-w-[360px] truncate text-xs text-muted-foreground/70"
                        title={row.dir}
                      >
                        {row.dir}
                      </div>
                    ) : null}
                  </div>
                </div>
                <Toggle
                  checked={enabled}
                  disabled={!settings}
                  onChange={() => toggleAgent(row.id)}
                  ariaLabel={`${enabled ? "停用" : "启用"} ${label}`}
                />
              </div>
            );
          })}
        </div>
      </SectionCard>

      <SectionCard
        icon={<PlusIcon className="h-4 w-4 text-primary" />}
        title="自定义 Agent"
        description="复用内置解析器统计其它 Agent:选一个与其数据格式相同的布局,指向对应目录即可(比如某个 Claude Code fork 的 projects 目录)"
      >
        <div className="divide-y divide-border/40">
          {(settings?.customAgents ?? []).length === 0 ? (
            <p className="px-4 py-3 text-xs text-muted-foreground">
              还没有自定义 Agent,用下面的表单添加。
            </p>
          ) : (
            (settings?.customAgents ?? []).map((c) => (
              <div
                key={c.id}
                className="flex items-center justify-between px-4 py-2.5"
              >
                <div className="min-w-0">
                  <div className="flex items-center gap-2 text-sm font-medium">
                    <span className="truncate">{c.name}</span>
                    <Badge tone="muted">{kindLabel(c.kind)}</Badge>
                  </div>
                  <div
                    className="mt-0.5 max-w-[420px] truncate text-xs text-muted-foreground"
                    title={c.dir}
                  >
                    {c.dir}
                  </div>
                </div>
                <button
                  type="button"
                  onClick={() => removeCustom(c.id)}
                  title="删除"
                  className="flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
                >
                  <TrashIcon className="h-4 w-4" />
                </button>
              </div>
            ))
          )}
        </div>
        <div className="space-y-2 border-t border-border/40 bg-background/40 p-4">
          <div className="grid gap-2 sm:grid-cols-3">
            <input
              value={cName}
              onChange={(e) => setCName(e.target.value)}
              placeholder="名称,如 CodeBuddy"
              className="h-8 rounded-lg border border-border bg-background px-2.5 text-xs outline-none focus:border-primary"
            />
            <select
              value={cKind}
              onChange={(e) =>
                setCKind(e.target.value as CustomAgentConfig["kind"])
              }
              className="h-8 rounded-lg border border-border bg-background px-2 text-xs outline-none focus:border-primary"
            >
              {KIND_OPTIONS.map((k) => (
                <option key={k.value} value={k.value}>
                  {k.label}
                </option>
              ))}
            </select>
            <input
              value={cDir}
              onChange={(e) => setCDir(e.target.value)}
              placeholder="数据目录,如 C:\Users\you\.codebuddy\projects"
              className="h-8 rounded-lg border border-border bg-background px-2.5 text-xs outline-none focus:border-primary"
            />
          </div>
          <div className="flex items-center justify-between gap-3">
            <p className="text-xs text-muted-foreground">
              {KIND_OPTIONS.find((k) => k.value === cKind)?.hint}
            </p>
            <button
              type="button"
              onClick={addCustom}
              disabled={!settings}
              className="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-50"
            >
              <PlusIcon className="h-3.5 w-3.5" />
              添加
            </button>
          </div>
        </div>
      </SectionCard>

      <SectionCard
        icon={<RefreshIcon className="h-4 w-4 text-primary" />}
        title="版本"
        description="从 GitHub Releases 检测最新版本(需要能访问 GitHub)"
      >
        <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3">
          <div className="min-w-0">
            <div className="text-sm font-medium">
              当前版本 {appVersion || "0.1.0"}
              {updateLatest ? (
                <span className="ml-2 rounded-md bg-orange-500/15 px-1.5 py-0.5 text-xs font-medium text-orange-500">
                  可更新到 v{updateLatest}
                </span>
              ) : null}
            </div>
            {updateMsg ? (
              <div
                className={`mt-1 text-xs ${
                  updateState === "error" ? "text-destructive" : "text-muted-foreground"
                }`}
              >
                {updateMsg}
              </div>
            ) : null}
          </div>
          <button
            type="button"
            onClick={() => void checkUpdate()}
            disabled={updateState === "checking"}
            className="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-xs font-medium transition-colors hover:bg-black/5 disabled:opacity-50 dark:hover:bg-white/5"
          >
            <RefreshIcon
              className={`h-3.5 w-3.5 ${updateState === "checking" ? "animate-spin" : ""}`}
            />
            检测更新
          </button>
        </div>
      </SectionCard>

      <SectionCard
        icon={<CoinsIcon className="h-4 w-4 text-primary" />}
        title="成本定价"
        description="填了定价的模型一律按你的价格重算成本(覆盖自带成本);没填的用自带成本或 0。单位 $/百万 tokens,按汇率折算展示"
      >
        <div className="space-y-3 p-4">
          <div className="flex flex-wrap items-center gap-3">
            <button
              type="button"
              onClick={() => void fetchModelsDev()}
              disabled={fetchState === "loading" || !settings}
              className="inline-flex h-8 items-center gap-1.5 rounded-lg bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-50"
            >
              <RefreshIcon
                className={`h-3.5 w-3.5 ${fetchState === "loading" ? "animate-spin" : ""}`}
              />
              从 models.dev 自动获取
            </button>
            <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
              币种
              <select
                value={settings?.currency ?? "CNY"}
                onChange={(e) => updateCurrency(e.target.value)}
                className="h-7 rounded-lg border border-border bg-background px-2 text-xs outline-none focus:border-primary"
              >
                <option value="CNY">¥ 人民币</option>
                <option value="USD">$ 美元</option>
              </select>
            </label>
            <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
              汇率(¥/$)
              <input
                type="number"
                step="0.1"
                min="0"
                value={settings?.exchangeRate ?? 7.2}
                onChange={(e) => updateExchangeRate(e.target.value)}
                onBlur={persistNow}
                className="h-7 w-20 rounded-lg border border-border bg-background px-2 text-xs tabular-nums outline-none focus:border-primary"
              />
            </label>
            <button
              type="button"
              onClick={() => void fetchFxRate()}
              disabled={fxState === "loading" || !settings}
              title="从 er-api / frankfurter 获取实时 USD→CNY 汇率"
              className="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-xs font-medium transition-colors hover:bg-black/5 disabled:opacity-50 dark:hover:bg-white/5"
            >
              获取实时汇率
            </button>
            {fxMsg ? (
              <span
                className={`text-xs ${
                  fxState === "error" ? "text-destructive" : "text-muted-foreground"
                }`}
              >
                {fxMsg}
              </span>
            ) : null}
            {fetchMsg ? (
              <span
                className={`text-xs ${
                  fetchState === "error" ? "text-destructive" : "text-muted-foreground"
                }`}
              >
                {fetchMsg}
              </span>
            ) : null}
          </div>

          <div className="overflow-x-auto">
            <table className="w-full min-w-[640px] text-xs">
              <thead>
                <tr className="border-b border-border/60 text-left text-muted-foreground">
                  <th className="py-1.5 pr-2 font-medium">模型</th>
                  <th className="py-1.5 pr-2 text-right font-medium">输入 $/M</th>
                  <th className="py-1.5 pr-2 text-right font-medium">输出 $/M</th>
                  <th className="py-1.5 pr-2 text-right font-medium">缓存读 $/M</th>
                  <th className="py-1.5 pr-2 text-right font-medium">缓存写 $/M</th>
                  <th className="py-1.5 text-right font-medium">操作</th>
                </tr>
              </thead>
              <tbody>
                {models.length === 0 ? (
                  <tr>
                    <td colSpan={6} className="py-3 text-muted-foreground">
                      暂无模型数据,先用 Agent 跑几轮再回来配置
                    </td>
                  </tr>
                ) : (
                  models.map((model) => {
                    const p: PriceEntry | undefined = settings?.pricing[model];
                    const cell = (field: keyof PriceEntry) => (
                      <input
                        type="number"
                        step="0.01"
                        min="0"
                        value={p ? String(p[field] ?? 0) : ""}
                        placeholder="0"
                        onChange={(e) => updatePrice(model, field, e.target.value)}
                        onBlur={persistNow}
                        className="h-7 w-20 rounded-lg border border-border bg-background px-2 text-right tabular-nums outline-none focus:border-primary"
                      />
                    );
                    return (
                      <tr key={model} className="border-b border-border/30">
                        <td className="max-w-[220px] truncate py-1.5 pr-2" title={model}>
                          {model}
                        </td>
                        <td className="py-1.5 pr-2 text-right">{cell("input")}</td>
                        <td className="py-1.5 pr-2 text-right">{cell("output")}</td>
                        <td className="py-1.5 pr-2 text-right">{cell("cacheRead")}</td>
                        <td className="py-1.5 pr-2 text-right">{cell("cacheWrite")}</td>
                        <td className="py-1.5 text-right">
                          <button
                            type="button"
                            onClick={() => clearPrice(model)}
                            disabled={!p}
                            title="清除该模型定价"
                            className="inline-flex h-7 w-7 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive disabled:opacity-30"
                          >
                            <TrashIcon className="h-3.5 w-3.5" />
                          </button>
                        </td>
                      </tr>
                    );
                  })
                )}
              </tbody>
            </table>
          </div>
        </div>
      </SectionCard>

      <SectionCard
        icon={<RefreshIcon className="h-4 w-4 text-primary" />}
        title="操作"
        description="手动触发扫描,完成后仪表盘数据自动更新"
      >
        <div className="flex flex-wrap items-center gap-3 p-4">
          <button
            type="button"
            onClick={handleRefresh}
            disabled={busyAction !== "none"}
            className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-xs font-medium transition-colors hover:bg-black/5 disabled:opacity-50 dark:hover:bg-white/5"
          >
            <RefreshIcon
              className={`h-3.5 w-3.5 ${busyAction === "refresh" ? "animate-spin" : ""}`}
            />
            立即刷新
          </button>
          <button
            type="button"
            onClick={handleFullRescan}
            disabled={busyAction !== "none"}
            className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-destructive/40 bg-background px-3 text-xs font-medium text-destructive transition-colors hover:bg-destructive/10 disabled:opacity-50"
          >
            <DatabaseIcon className="h-3.5 w-3.5" />
            全量重扫(清缓存重建)
          </button>
          {busyAction !== "none" ? (
            <span className="text-xs text-muted-foreground">
              正在扫描,完成后数据自动更新…
            </span>
          ) : null}
        </div>
      </SectionCard>

      <SectionCard
        icon={<SunIcon className="h-4 w-4 text-primary" />}
        title="外观"
        description="主题切换即时生效,并保存在本机"
      >
        <div className="flex items-center justify-between px-4 py-3">
          <div>
            <div className="text-sm font-medium">主题</div>
            <div className="mt-0.5 text-xs text-muted-foreground">
              当前:{theme === "dark" ? "暗色" : "亮色"}
            </div>
          </div>
          <div className="flex items-center gap-1 rounded-xl bg-muted p-1">
            <button
              type="button"
              onClick={() => applyTheme("dark")}
              className={`flex h-7 items-center gap-1 rounded-lg px-2.5 text-xs font-medium transition-colors ${
                theme === "dark"
                  ? "bg-background shadow-sm text-foreground"
                  : "text-muted-foreground hover:bg-black/5 dark:hover:bg-white/5"
              }`}
            >
              <MoonIcon className="h-3.5 w-3.5" />
              暗色
            </button>
            <button
              type="button"
              onClick={() => applyTheme("light")}
              className={`flex h-7 items-center gap-1 rounded-lg px-2.5 text-xs font-medium transition-colors ${
                theme === "light"
                  ? "bg-background shadow-sm text-foreground"
                  : "text-muted-foreground hover:bg-black/5 dark:hover:bg-white/5"
              }`}
            >
              <SunIcon className="h-3.5 w-3.5" />
              亮色
            </button>
          </div>
        </div>
      </SectionCard>
    </div>
  );
}
