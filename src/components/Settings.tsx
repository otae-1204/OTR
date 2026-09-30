import { useEffect, useState, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AGENT_LABELS,
  api,
  type AgentStatus,
  type CustomAgentConfig,
  type LimitAccount,
  type PriceEntry,
  type Settings as AppSettings,
  DEFAULT_LIMIT_PROVIDERS,
  LIMIT_PROVIDER_LABELS,
} from "../api/bindings";
import { fmtTokens } from "../lib/format";
import {
  ChevronDownIcon,
  CoinsIcon,
  DatabaseIcon,
  GaugeIcon,
  MoonIcon,
  PlusIcon,
  RefreshIcon,
  SunIcon,
  TrashIcon,
} from "./icons";
import { AgentIcon, usesImageIcon } from "./AgentIcon";
import { SelectMenu } from "./SelectMenu";
import { compareVersions, fetchLatestVersion, fetchUsdCnyRate } from "../lib/remote";
import { useTheme } from "../theme/ThemeProvider";
import type { ThemeEntry, ThemeMode } from "../theme/types";

const KNOWN_AGENTS = [
  "dsh",
  "claude-code",
  "codex",
  "zcode",
  "opencode",
  "pi",
  "cursor",
];
const KIND_OPTIONS: { value: string; label: string }[] = [
  { value: "claude-code", label: "Claude Code 布局" },
  { value: "codex", label: "Codex 布局" },
  { value: "zcode", label: "ZCode 布局" },
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
      data-theme-part="badge"
      className={`rounded-md px-1.5 py-0.5 text-xs ${
        tone === "good"
          ? "bg-success/10 text-success-text"
          : "bg-muted text-muted-foreground"
      }`}
    >
      {children}
    </span>
  );
}

/**
 * 自制开关:w-11 h-6 轨道,开 = bg-success、关 = mutedForeground 30%;
 * 滑块颜色是主题的 switchThumb / switchThumbOff,平移动画。设置页所有开关都用它。
 */
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
      data-theme-part="switch"
      data-theme-state={checked ? "selected" : undefined}
      className={`relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors duration-200 ${
        checked ? "bg-success" : "bg-muted-foreground/30"
      } disabled:cursor-not-allowed disabled:opacity-50`}
    >
      <span
        data-theme-part="switch-thumb"
        className={`inline-block h-5 w-5 rounded-full shadow transition-[transform,background-color] duration-200 ${
          checked ? "translate-x-5 bg-switch-thumb" : "translate-x-0.5 bg-switch-thumb-off"
        }`}
      />
    </button>
  );
}

const PROVIDER_LABELS: Record<string, string> = {
  cursor: "Cursor",
  codex: "Codex CLI",
  deepseek: "DeepSeek",
  qwen: "Qwen",
  stepfun: "StepFun",
};

/** home 模式靠本地 CLI 会话(要目录),key 模式靠 API key(表单里直接粘贴) */
const PROVIDER_MODE: Record<string, "home" | "key"> = {
  cursor: "home",
  codex: "home",
  deepseek: "key",
  qwen: "key",
  stepfun: "key",
};

const PROVIDER_ORDER = ["cursor", "codex", "deepseek", "qwen", "stepfun"] as const;

const BUILTIN_API_KEY: Record<string, { name: string; placeholder: string }> = {
  deepseek: { name: "DEEPSEEK_API_KEY", placeholder: "更新 API Key" },
};

const FIELD_CLASS =
  "h-8 w-full rounded-lg border border-border bg-background px-2.5 text-xs outline-none focus:border-primary";

function newLimitAccountId(provider: string, existing: string[]): string {
  for (let i = 0; i < 8; i++) {
    const id = `${provider}-${Date.now().toString(36)}-${Math.random()
      .toString(36)
      .slice(2, 6)}`;
    if (!existing.includes(id)) return id;
  }
  return `${provider}-${Date.now().toString(36)}`;
}

function accountStatus(a: LimitAccount): string {
  if (a.provider === "cursor" || a.provider === "codex") {
    if (a.builtin) return "内置 · 本机会话";
    return a.home?.trim() || "未选择目录";
  }
  if (a.provider === "qwen") {
    const cookie = a.cookiePresent ? "Cookie 已配置" : "Cookie 未配置";
    const key = a.keyPresent ? "Key 已配置" : "Key 未配置";
    return a.builtin ? `内置 · ${cookie} · ${key}` : `${cookie} · ${key}`;
  }
  if (a.provider === "stepfun") {
    const cookie = a.cookiePresent ? "订阅已配置" : "订阅未配置";
    const key = a.keyPresent ? "余额已配置" : "余额未配置";
    return a.builtin ? `内置 · ${cookie} · ${key}` : `${cookie} · ${key}`;
  }
  const key = a.keyPresent ? "API Key 已配置" : "API Key 未配置";
  return a.builtin ? `内置 · ${key}` : key;
}

/**
 * 额度来源:每个 Provider 一块。加同类账号只填显示名,再贴密钥或选目录。
 * id 和凭据条目名由程序生成,不出现在界面上。
 */
function LimitSources({ onChanged }: { onChanged: () => void }) {
  const [accounts, setAccounts] = useState<LimitAccount[] | null>(null);
  const [providers, setProviders] = useState<string[]>([]);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [openProvider, setOpenProvider] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [draftLabel, setDraftLabel] = useState("");
  const [draftHome, setDraftHome] = useState("");
  const [draftKey, setDraftKey] = useState("");
  const [draftCookie, setDraftCookie] = useState("");
  const [builtinDraft, setBuiltinDraft] = useState<Record<string, string>>({});

  const reload = () => {
    void api
      .listLimitAccounts()
      .then(setAccounts)
      .catch((e) => setErr(String(e)));
    void api
      .getSettings()
      .then((s) => setProviders(s.limitProviders ?? DEFAULT_LIMIT_PROVIDERS))
      .catch(() => undefined);
  };

  useEffect(reload, []);

  const refreshLimits = () => {
    void api.refreshLimits().catch(() => undefined);
  };

  const toggleProvider = async (p: string, on: boolean) => {
    setErr(null);
    try {
      await api.setLimitProvider(p, on);
      setProviders((prev) => (on ? [...prev, p] : prev.filter((x) => x !== p)));
      if (!on) setOpenProvider((cur) => (cur === p ? null : cur));
      onChanged();
      reload();
      if (on) refreshLimits();
    } catch (e) {
      setErr(String(e));
    }
  };

  const openAdd = (provider: string) => {
    setOpenProvider(provider);
    setDraftLabel("");
    setDraftHome("");
    setDraftKey("");
    setDraftCookie("");
    setErr(null);
    setMsg(null);
  };

  const canAdd = (provider: string) => {
    if (!draftLabel.trim() || adding) return false;
    if (PROVIDER_MODE[provider] === "home") return draftHome.trim().length > 0;
    if (provider === "stepfun" || provider === "qwen") {
      return draftCookie.trim().length > 0 || draftKey.trim().length > 0;
    }
    return draftKey.trim().length > 0;
  };

  const addAccount = async (provider: string) => {
    if (!canAdd(provider)) return;
    setErr(null);
    setMsg(null);
    setAdding(true);
    const label = draftLabel.trim();
    const mode = PROVIDER_MODE[provider];
    try {
      await api.saveLimitAccount(
        {
          id: newLimitAccountId(
            provider,
            (accounts ?? []).map((a) => a.id),
          ),
          provider,
          label,
          plan: "",
          home: mode === "home" ? draftHome.trim() : null,
          secretRef: null,
          cookieRef: null,
        },
        {
          apiKey: mode === "key" ? draftKey.trim() : null,
          consoleCookie:
            provider === "stepfun" || provider === "qwen" ? draftCookie.trim() : null,
        },
      );
      setMsg(`已添加 ${label}`);
      setOpenProvider(null);
      setDraftLabel("");
      setDraftHome("");
      setDraftKey("");
      setDraftCookie("");
      onChanged();
      reload();
      refreshLimits();
    } catch (e) {
      setErr(String(e));
    } finally {
      setAdding(false);
    }
  };

  const removeAccount = async (account: LimitAccount) => {
    if (
      !window.confirm(
        `删除「${account.label}」?已保存的密钥会一起清掉。`,
      )
    ) {
      return;
    }
    setErr(null);
    try {
      await api.deleteLimitAccount(account.id);
      setMsg(`已删除 ${account.label}`);
      onChanged();
      reload();
      refreshLimits();
    } catch (e) {
      setErr(String(e));
    }
  };

  const saveBuiltin = async (name: string) => {
    setErr(null);
    setMsg(null);
    const value = (builtinDraft[name] ?? "").trim();
    if (!value) return;
    try {
      await api.saveLimitCredential(name, value);
      setBuiltinDraft((d) => ({ ...d, [name]: "" }));
      setMsg("已保存");
      reload();
      refreshLimits();
    } catch (e) {
      setErr(String(e));
    }
  };

  const clearBuiltin = async (name: string) => {
    setErr(null);
    try {
      await api.deleteLimitCredential(name);
      setMsg("已清除");
      reload();
      refreshLimits();
    } catch (e) {
      setErr(String(e));
    }
  };

  const browseHome = async () => {
    setErr(null);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "选择 profile 目录",
      });
      if (typeof selected === "string") setDraftHome(selected);
    } catch (e) {
      setErr(String(e));
    }
  };

  const openGuide = (provider: string) => {
    setErr(null);
    void api.openCookieGuide(provider).catch((e) => setErr(String(e)));
  };

  return (
    <div>
      <div className="divide-y divide-border/40">
        {PROVIDER_ORDER.map((p) => {
          const on = providers.includes(p);
          const rows = (accounts ?? []).filter((a) => a.provider === p);
          return (
            <div key={p}>
              <div className="flex items-center justify-between gap-3 px-4 py-3">
                <div className="flex min-w-0 items-center gap-2">
                  <div className="truncate text-sm font-medium">
                    {PROVIDER_LABELS[p]}
                  </div>
                  {p === "qwen" || p === "stepfun" ? (
                    <button
                      type="button"
                      onClick={() => openGuide(p)}
                      data-theme-part="button"
                      className="inline-flex h-6 shrink-0 items-center rounded-md border border-border bg-background px-2 text-11px font-medium transition-colors hover:border-primary/60 hover:text-primary"
                    >
                      教程
                    </button>
                  ) : null}
                </div>
                <Toggle
                  checked={on}
                  onChange={() => void toggleProvider(p, !on)}
                  ariaLabel={`${on ? "关闭" : "开启"} ${PROVIDER_LABELS[p]}`}
                />
              </div>
              {on ? (
                <div className="border-t border-border/40 bg-background/40">
                  {accounts == null ? (
                    <p className="px-4 py-3 text-xs text-muted-foreground">
                      正在读取账号…
                    </p>
                  ) : (
                    <div className="divide-y divide-border/40">
                      {rows.map((a) => (
                        <div key={a.id} className="px-4 py-2.5">
                          <div className="flex items-start justify-between gap-3">
                            <div className="min-w-0">
                              <div className="truncate text-sm font-medium">
                                {a.label}
                              </div>
                              <div
                                className="mt-0.5 max-w-[420px] truncate text-xs text-muted-foreground"
                                title={accountStatus(a)}
                              >
                                {accountStatus(a)}
                              </div>
                            </div>
                            {a.builtin ? null : (
                              <button
                                type="button"
                                title="删除该账号"
                                onClick={() => void removeAccount(a)}
                                className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
                              >
                                <TrashIcon className="h-4 w-4" />
                              </button>
                            )}
                          </div>
                          {a.builtin && BUILTIN_API_KEY[a.provider] ? (
                            <BuiltinSecret
                              placeholder={BUILTIN_API_KEY[a.provider].placeholder}
                              value={builtinDraft[BUILTIN_API_KEY[a.provider].name] ?? ""}
                              present={!!a.keyPresent}
                              onChange={(value) =>
                                setBuiltinDraft((d) => ({
                                  ...d,
                                  [BUILTIN_API_KEY[a.provider].name]: value,
                                }))
                              }
                              onSave={() =>
                                void saveBuiltin(BUILTIN_API_KEY[a.provider].name)
                              }
                              onClear={() =>
                                void clearBuiltin(BUILTIN_API_KEY[a.provider].name)
                              }
                            />
                          ) : null}
                          {a.builtin && a.provider === "qwen" ? (
                            <div className="space-y-2">
                              <BuiltinSecret
                                placeholder="百炼控制台 Cookie"
                                value={builtinDraft.QWEN_CONSOLE_COOKIE ?? ""}
                                present={!!a.cookiePresent}
                                onChange={(value) =>
                                  setBuiltinDraft((d) => ({
                                    ...d,
                                    QWEN_CONSOLE_COOKIE: value,
                                  }))
                                }
                                onSave={() => void saveBuiltin("QWEN_CONSOLE_COOKIE")}
                                onClear={() => void clearBuiltin("QWEN_CONSOLE_COOKIE")}
                              />
                              <BuiltinSecret
                                placeholder="Coding Plan API Key(可选)"
                                value={builtinDraft.QWEN_CODING_PLAN_API_KEY ?? ""}
                                present={!!a.keyPresent}
                                onChange={(value) =>
                                  setBuiltinDraft((d) => ({
                                    ...d,
                                    QWEN_CODING_PLAN_API_KEY: value,
                                  }))
                                }
                                onSave={() =>
                                  void saveBuiltin("QWEN_CODING_PLAN_API_KEY")
                                }
                                onClear={() =>
                                  void clearBuiltin("QWEN_CODING_PLAN_API_KEY")
                                }
                              />
                            </div>
                          ) : null}
                          {a.builtin && a.provider === "stepfun" ? (
                            <div className="space-y-2">
                              <BuiltinSecret
                                placeholder="更新控制台 Cookie"
                                value={builtinDraft.STEPFUN_CONSOLE_COOKIE ?? ""}
                                present={!!a.cookiePresent}
                                onChange={(value) =>
                                  setBuiltinDraft((d) => ({
                                    ...d,
                                    STEPFUN_CONSOLE_COOKIE: value,
                                  }))
                                }
                                onSave={() =>
                                  void saveBuiltin("STEPFUN_CONSOLE_COOKIE")
                                }
                                onClear={() =>
                                  void clearBuiltin("STEPFUN_CONSOLE_COOKIE")
                                }
                              />
                              <BuiltinSecret
                                placeholder="更新 API Key(按量余额,可选)"
                                value={builtinDraft.STEPFUN_API_KEY ?? ""}
                                present={!!a.keyPresent}
                                onChange={(value) =>
                                  setBuiltinDraft((d) => ({
                                    ...d,
                                    STEPFUN_API_KEY: value,
                                  }))
                                }
                                onSave={() => void saveBuiltin("STEPFUN_API_KEY")}
                                onClear={() => void clearBuiltin("STEPFUN_API_KEY")}
                              />
                            </div>
                          ) : null}
                        </div>
                      ))}
                    </div>
                  )}
                  {openProvider === p ? (
                    <div className="space-y-2 border-t border-border/40 px-4 py-3">
                      <input
                        value={draftLabel}
                        onChange={(e) => setDraftLabel(e.target.value)}
                        placeholder="显示名,如 工作"
                        data-theme-part="input"
                        className={FIELD_CLASS}
                      />
                      {PROVIDER_MODE[p] === "home" ? (
                        <div className="flex gap-2">
                          <input
                            value={draftHome}
                            onChange={(e) => setDraftHome(e.target.value)}
                            placeholder="profile 目录"
                            data-theme-part="input"
                            className="h-8 min-w-0 flex-1 rounded-lg border border-border bg-background px-2.5 font-mono text-xs outline-none focus:border-primary"
                          />
                          <button
                            type="button"
                            onClick={() => void browseHome()}
                            data-theme-part="button"
                            className="inline-flex h-8 shrink-0 items-center rounded-lg border border-border bg-background px-3 text-xs font-medium transition-colors hover:bg-overlay/5"
                          >
                            浏览
                          </button>
                        </div>
                      ) : p === "qwen" ? (
                        <>
                          <input
                            type="password"
                            autoComplete="off"
                            value={draftCookie}
                            onChange={(e) => setDraftCookie(e.target.value)}
                            placeholder="百炼控制台 Cookie"
                            data-theme-part="input"
                            className={FIELD_CLASS}
                          />
                          <input
                            type="password"
                            autoComplete="off"
                            value={draftKey}
                            onChange={(e) => setDraftKey(e.target.value)}
                            placeholder="Coding Plan API Key(可选)"
                            data-theme-part="input"
                            className={FIELD_CLASS}
                          />
                        </>
                      ) : p === "stepfun" ? (
                        <>
                          <input
                            type="password"
                            autoComplete="off"
                            value={draftCookie}
                            onChange={(e) => setDraftCookie(e.target.value)}
                            placeholder="控制台 Cookie(订阅额度)"
                            data-theme-part="input"
                            className={FIELD_CLASS}
                          />
                          <input
                            type="password"
                            autoComplete="off"
                            value={draftKey}
                            onChange={(e) => setDraftKey(e.target.value)}
                            placeholder="API Key(按量余额,可选)"
                            data-theme-part="input"
                            className={FIELD_CLASS}
                          />
                        </>
                      ) : (
                        <input
                          type="password"
                          autoComplete="off"
                          value={draftKey}
                          onChange={(e) => setDraftKey(e.target.value)}
                          placeholder="API Key"
                          data-theme-part="input"
                          className={FIELD_CLASS}
                        />
                      )}
                      <div className="flex justify-end gap-2">
                          <button
                            type="button"
                            onClick={() => setOpenProvider(null)}
                            data-theme-part="button"
                            className="inline-flex h-8 items-center rounded-lg border border-border bg-background px-3 text-xs font-medium transition-colors hover:bg-overlay/5"
                          >
                            取消
                          </button>
                          <button
                            type="button"
                            disabled={!canAdd(p)}
                            onClick={() => void addAccount(p)}
                            data-theme-part="button-primary"
                            className="inline-flex h-8 items-center gap-1.5 rounded-lg bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-40"
                          >
                            <PlusIcon className="h-3.5 w-3.5" />
                            添加
                          </button>
                      </div>
                    </div>
                  ) : (
                    <div className="border-t border-border/40 px-4 py-2.5">
                      <button
                        type="button"
                        onClick={() => openAdd(p)}
                        className="inline-flex h-8 items-center gap-1.5 text-xs font-medium text-primary"
                      >
                        <PlusIcon className="h-3.5 w-3.5" />
                        添加账号
                      </button>
                    </div>
                  )}
                </div>
              ) : null}
            </div>
          );
        })}
      </div>
      {msg ? <p className="px-4 py-2 text-xs text-success-label">{msg}</p> : null}
      {err ? <p className="px-4 py-2 text-xs text-danger-text">{err}</p> : null}
    </div>
  );
}

function BuiltinSecret({
  placeholder,
  value,
  present,
  onChange,
  onSave,
  onClear,
}: {
  placeholder: string;
  value: string;
  present: boolean;
  onChange: (value: string) => void;
  onSave: () => void;
  onClear: () => void;
}) {
  return (
    <div className="mt-2 flex flex-wrap items-center gap-2">
      <input
        type="password"
        autoComplete="off"
        placeholder={placeholder}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        data-theme-part="input"
        className="h-8 min-w-[180px] flex-1 rounded-lg border border-border bg-background px-2.5 text-xs outline-none focus:border-primary"
      />
      <button
        type="button"
        onClick={onSave}
        disabled={!value.trim()}
        data-theme-part="button-primary"
        className="inline-flex h-8 items-center rounded-lg bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-40"
      >
        保存
      </button>
      <button
        type="button"
        onClick={onClear}
        disabled={!present}
        data-theme-part="button"
        className="inline-flex h-8 items-center rounded-lg border border-border bg-background px-2.5 text-xs text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive disabled:opacity-30"
      >
        清除
      </button>
    </div>
  );
}

function SectionCard({
  icon,
  title,
  children,
  collapsible = false,
  summary,
}: {
  icon: ReactNode;
  title: string;
  children: ReactNode;
  /** 可折叠。默认收起,展开与否记在本机,下次打开设置还是这个状态 */
  collapsible?: boolean;
  /** 收起时的一行摘要,比如「3 个来源已开启」 */
  summary?: string;
}) {
  const [open, setOpen] = useState(() => {
    if (!collapsible) return true;
    try {
      return localStorage.getItem(`token-show-fold:${title}`) === "1";
    } catch {
      return false;
    }
  });

  const toggle = () => {
    setOpen((v) => {
      const next = !v;
      try {
        localStorage.setItem(`token-show-fold:${title}`, next ? "1" : "0");
      } catch {
        // 写不进去就只在这次打开里生效
      }
      return next;
    });
  };

  const head = (
    <>
      <h3 data-theme-part="card-title" className="flex items-center gap-1.5 text-sm font-semibold">
        {icon}
        {title}
      </h3>
      {collapsible && !open && summary ? (
        <p className="mt-0.5 text-xs text-muted-foreground">{summary}</p>
      ) : null}
    </>
  );

  return (
    <section
      data-theme-part="card settings-section"
      className="overflow-hidden rounded-xl border border-border bg-card transition-all duration-300 hover:border-primary/60 hover:shadow-sm"
    >
      {collapsible ? (
        <button
          type="button"
          onClick={toggle}
          aria-expanded={open}
          className="flex w-full items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-overlay/5"
        >
          <div className="min-w-0 flex-1">{head}</div>
          <ChevronDownIcon
            className={`h-4 w-4 shrink-0 text-muted-foreground transition-transform ${
              open ? "" : "-rotate-90"
            }`}
          />
        </button>
      ) : (
        <div className="border-b border-border/40 px-4 py-3">{head}</div>
      )}
      {/* 收起时藏起来但不卸载:定价表和账号表单的未保存输入还在 */}
      <div
        className={
          collapsible ? (open ? "border-t border-border/40" : "hidden") : undefined
        }
      >
        {children}
      </div>
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
  // 主题与深浅模式由 ThemeProvider 应用到 DOM;这里只负责把选择写进设置
  const themeCtx = useTheme();
  const theme = themeCtx.mode;
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

  /** 深浅按钮改的是**偏好**模式;theme 字段同步记下生效模式(旧版本应用只认它) */
  const applyTheme = (mode: ThemeMode) => {
    const outcome = themeCtx.setMode(mode);
    if (settings) {
      const next = { ...settings, preferredMode: outcome.preferredMode, theme: outcome.mode };
      setSettings(next);
      void api.saveSettings(next).catch(() => undefined);
    }
  };

  const applyThemeId = (id: string) => {
    const outcome = themeCtx.selectTheme(id);
    if (settings) {
      // 单模式主题只改变生效模式,偏好模式原样保留,换回双模式主题时恢复
      const next = {
        ...settings,
        themeId: id,
        preferredMode: outcome.preferredMode,
        theme: outcome.mode,
      };
      setSettings(next);
      void api.saveSettings(next).catch(() => undefined);
    }
  };

  // 托盘「恢复默认主题」由 Rust 直接改写设置文件;把本页快照里的外观字段同步过来,
  // 否则之后在本页保存别的设置时会把旧的 themeId 写回去
  const resetSeq = themeCtx.resetSeq;
  useEffect(() => {
    if (resetSeq === 0) return;
    let active = true;
    api
      .getSettings()
      .then((fresh) => {
        if (!active) return;
        setSettings((cur) =>
          cur
            ? {
                ...cur,
                themeId: fresh.themeId,
                theme: fresh.theme,
                preferredMode: fresh.preferredMode,
              }
            : fresh,
        );
      })
      .catch((err) => console.error("[Settings] getSettings 失败", err));
    return () => {
      active = false;
    };
  }, [resetSeq]);

  const [themeReloading, setThemeReloading] = useState(false);
  const reloadThemes = async () => {
    setThemeReloading(true);
    try {
      await themeCtx.reload();
    } finally {
      setThemeReloading(false);
    }
  };
  /** 当前生效主题的条目(用来判断它支持哪些模式) */
  const activeEntry: ThemeEntry | undefined = themeCtx.entries.find(
    (e) => e.id === themeCtx.theme.id && e.manifest,
  );
  /** 当前主题只提供一种模式时是那个模式;此时深浅按钮整体禁用(偏好保留,换回双模式主题时恢复) */
  const singleMode: ThemeMode | null =
    activeEntry && activeEntry.modes.length === 1 ? activeEntry.modes[0] : null;
  const modeLabel = (m: ThemeMode) => (m === "dark" ? "暗色" : "亮色");
  const modeLockedTip = singleMode
    ? `当前主题只提供${modeLabel(singleMode)};换回双模式主题后可修改深浅偏好`
    : undefined;
  const selectedExists = themeCtx.entries.some(
    (e) => e.id === themeCtx.selectedId && e.manifest,
  );
  /** 有诊断信息的用户主题(错误或警告),列在主题选择下面 */
  const themeIssues = themeCtx.entries.filter(
    (e) => e.source === "user" && e.diagnostics.length > 0,
  );

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
      } else if (compareVersions(latest, cur) > 0) {
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
    setSettings({
      ...settings,
      pricing: { ...settings.pricing, [model]: next },
      // 手工改过的定价标成 manual:下次从 models.dev 同步时会先问要不要覆盖
      pricingSource: { ...settings.pricingSource, [model]: "manual" },
    });
  };

  /**
   * 高峰单价:填一个数就按"平价 × 2"补出整套高峰档(DeepSeek 官方就是 2 倍),
   * 留空则清掉峰谷、退回纯平价。这样用户只需要动一个格子。
   */
  const updatePeak = (model: string, raw: string) => {
    if (!settings) return;
    const cur = settings.pricing[model] ?? {
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
    };
    const v = parseFloat(raw);
    const peak =
      Number.isFinite(v) && v >= 0
        ? {
            input: v,
            output: cur.output * 2,
            cacheRead: cur.cacheRead * 2,
            cacheWrite: cur.cacheWrite * 2,
          }
        : null;
    const next: PriceEntry = { ...cur, peak };
    setSettings({
      ...settings,
      pricing: { ...settings.pricing, [model]: next },
      pricingSource: { ...settings.pricingSource, [model]: "manual" },
    });
  };

  const persistNow = () => {
    if (!settings) return;
    void api.saveSettings(settings).catch(() => undefined);
  };

  const clearPrice = (model: string) => {
    if (!settings) return;
    const pricing = { ...settings.pricing };
    delete pricing[model];
    const pricingSource = { ...settings.pricingSource };
    delete pricingSource[model];
    const next = { ...settings, pricing, pricingSource };
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
      const pick = (model: string): { provider: string; entry: PriceEntry } | null => {
        const key = model.toLowerCase();
        const candidates = [key, model.split("/").pop()?.toLowerCase() ?? ""].filter(Boolean);
        for (const c of candidates) {
          const list = catalog.get(c);
          if (!list || list.length === 0) continue;
          const fam = familyProvider(c);
          const chosen = (fam && list.find((x) => x.provider === fam)) || list[0];
          return chosen;
        }
        return null;
      };
      const samePrice = (a: PriceEntry, b: PriceEntry) =>
        a.input === b.input &&
        a.output === b.output &&
        a.cacheRead === b.cacheRead &&
        a.cacheWrite === b.cacheWrite;

      const picked = models
        .map((model) => ({ model, found: pick(model) }))
        .filter(
          (
            x,
          ): x is {
            model: string;
            found: { provider: string; entry: PriceEntry };
          } => x.found !== null,
        );
      // 已经填过定价、且不是上一次 models.dev 同步写入的那些:官方价与它不同就先问用户。
      // 以前是 next[model] = found 静默覆盖,用户自己填的价格会被无声抹掉。
      const conflicts = picked.filter(({ model, found }) => {
        const cur = settings.pricing[model];
        if (!cur) return false;
        const src = (settings.pricingSource ?? {})[model];
        if (src && src.startsWith("models.dev:")) return false;
        return !samePrice(cur, found.entry);
      });
      let keepMine = false;
      if (conflicts.length > 0) {
        const names = conflicts
          .slice(0, 12)
          .map((c) => c.model)
          .join("\n");
        const more =
          conflicts.length > 12 ? `\n…还有 ${conflicts.length - 12} 个` : "";
        keepMine = !window.confirm(
          `以下 ${conflicts.length} 个模型你已经填过定价,与官方价不同:\n\n${names}${more}\n\n` +
            "「确定」= 用官方价覆盖这些模型\n「取消」= 保留你填的定价,只同步其余模型",
        );
      }
      const skip = new Set(keepMine ? conflicts.map((c) => c.model) : []);
      const next = { ...settings.pricing };
      const nextSource = { ...(settings.pricingSource ?? {}) };
      let matched = 0;
      for (const { model, found } of picked) {
        if (skip.has(model)) continue;
        next[model] = found.entry;
        nextSource[model] = `models.dev:${found.provider}`;
        matched++;
      }
      await persist({ ...settings, pricing: next, pricingSource: nextSource });
      setFetchState("ok");
      setFetchMsg(
        `已匹配 ${matched} / ${models.length} 个本地模型的官方定价` +
          (skip.size > 0 ? `(保留了 ${skip.size} 个你手动设置的)` : ""),
      );
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
      <h2 data-theme-part="page-title" className="text-lg font-semibold">设置</h2>

      <SectionCard
        icon={<DatabaseIcon className="h-4 w-4 text-primary" />}
        title="数据源"
      >
        <div>
          {rows.map((row) => {
            const status = agents.find((a) => a.id === row.id);
            const detected = status?.detected ?? false;
            const enabled = settings
              ? settings.enabledAgents.includes(row.id)
              : (status?.enabled ?? false);
            const color = themeCtx.agentColor(row.id);
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
              data-theme-part="input"
              className="h-8 rounded-lg border border-border bg-background px-2.5 text-xs outline-none focus:border-primary"
            />
            <SelectMenu
              value={cKind}
              onChange={(v) => setCKind(v as CustomAgentConfig["kind"])}
              ariaLabel="数据布局"
              className="h-8 w-full"
              options={KIND_OPTIONS}
            />
            <input
              value={cDir}
              onChange={(e) => setCDir(e.target.value)}
              placeholder={"数据目录,如 C:\\Users\\you\\.codebuddy\\projects"}
              data-theme-part="input"
              className="h-8 rounded-lg border border-border bg-background px-2.5 text-xs outline-none focus:border-primary"
            />
          </div>
          <div className="flex justify-end">
            <button
              type="button"
              onClick={addCustom}
              disabled={!settings}
              data-theme-part="button-primary"
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
      >
        <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3">
          <div className="min-w-0">
            <div className="text-sm font-medium">
              当前版本 {appVersion || "0.1.0"}
              {updateLatest ? (
                <span data-theme-part="badge" className="ml-2 rounded-md bg-notice/15 px-1.5 py-0.5 text-xs font-medium text-notice">
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
            data-theme-part="button"
            className="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-xs font-medium transition-colors hover:bg-overlay/5 disabled:opacity-50"
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
        collapsible
        summary={
          settings
            ? `${models.length} 个模型 · ${Object.keys(settings.pricing).length} 个已定价`
            : undefined
        }
      >
        <div className="space-y-3 p-4">
          <div className="flex flex-wrap items-center gap-3">
            <button
              type="button"
              onClick={() => void fetchModelsDev()}
              disabled={fetchState === "loading" || !settings}
              data-theme-part="button-primary"
              className="inline-flex h-8 items-center gap-1.5 rounded-lg bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-50"
            >
              <RefreshIcon
                className={`h-3.5 w-3.5 ${fetchState === "loading" ? "animate-spin" : ""}`}
              />
              从 models.dev 自动获取
            </button>
            <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
              币种
              <SelectMenu
                value={settings?.currency ?? "CNY"}
                onChange={updateCurrency}
                ariaLabel="币种"
                className="h-7"
                options={[
                  { value: "CNY", label: "¥ 人民币" },
                  { value: "USD", label: "$ 美元" },
                ]}
              />
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
                data-theme-part="input"
                className="h-7 w-20 rounded-lg border border-border bg-background px-2 text-xs tabular-nums outline-none focus:border-primary"
              />
            </label>
            <button
              type="button"
              onClick={() => void fetchFxRate()}
              disabled={fxState === "loading" || !settings}
              title="从 er-api / frankfurter 获取实时 USD→CNY 汇率"
              data-theme-part="button"
              className="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-xs font-medium transition-colors hover:bg-overlay/5 disabled:opacity-50"
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
            <table data-theme-part="table" className="w-full min-w-[640px] text-xs">
              {/* 表头 / 行上重复写 text-xs:单元格的字号是继承来的,钩子元素自己带字号类,
                  主题 css 的 font-size 倍率(docs §14.3)才作用得到 */}
              <thead data-theme-part="table-head" className="text-xs">
                <tr className="border-b border-border/60 text-left text-muted-foreground">
                  <th className="py-1.5 pr-2 font-medium">模型</th>
                  <th className="py-1.5 pr-2 text-right font-medium">输入 $/M</th>
                  <th className="py-1.5 pr-2 text-right font-medium">输出 $/M</th>
                  <th className="py-1.5 pr-2 text-right font-medium">缓存读 $/M</th>
                  <th className="py-1.5 pr-2 text-right font-medium">缓存写 $/M</th>
                  <th
                    className="py-1.5 pr-2 text-right font-medium"
                    title="DeepSeek 等有峰谷分时计价的模型:高峰时段(北京时间工作日 9-12、14-18 点)的单价。留空表示按平价计"
                  >
                    高峰 $/M
                  </th>
                  <th className="py-1.5 text-right font-medium">操作</th>
                </tr>
              </thead>
              <tbody>
                {models.length === 0 ? (
                  <tr>
                    <td colSpan={7} className="py-3 text-muted-foreground">
                      暂无模型数据,先用 Agent 跑几轮再回来配置
                    </td>
                  </tr>
                ) : (
                  models.map((model) => {
                    const p: PriceEntry | undefined = settings?.pricing[model];
                    const src = settings?.pricingSource?.[model];
                    // 定价来源提示:用户要能一眼看出这个价格是官方拉的、自己填的,还是根本没填
                    const sourceLabel = !p
                      ? "无定价 · 用自带成本或 0"
                      : src === "manual"
                        ? "手动填写"
                        : src?.startsWith("curated:")
                          ? `内置修正价 · ${src.slice("curated:".length)}`
                          : src?.startsWith("models.dev:")
                            ? `官方 · ${src.slice("models.dev:".length)}`
                            : "来源未知";
                    const cell = (field: keyof PriceEntry) => (
                      <input
                        type="number"
                        step="0.01"
                        min="0"
                        value={p ? String(p[field] ?? 0) : ""}
                        placeholder="0"
                        onChange={(e) => updatePrice(model, field, e.target.value)}
                        onBlur={persistNow}
                        data-theme-part="input"
                        className="h-7 w-20 rounded-lg border border-border bg-background px-2 text-right tabular-nums outline-none focus:border-primary"
                      />
                    );
                    return (
                      <tr
                        key={model}
                        data-theme-part="table-row"
                        className="border-b border-border/30 text-xs"
                      >
                        <td className="max-w-[220px] py-1.5 pr-2" title={model}>
                          <div className="truncate">{model}</div>
                          <div
                            className={`mt-0.5 text-10px ${
                              !p
                                ? "text-muted-foreground/50"
                                : src === "manual"
                                  ? "text-warning-label"
                                  : "text-muted-foreground/70"
                            }`}
                          >
                            {sourceLabel}
                          </div>
                        </td>
                        <td className="py-1.5 pr-2 text-right">{cell("input")}</td>
                        <td className="py-1.5 pr-2 text-right">{cell("output")}</td>
                        <td className="py-1.5 pr-2 text-right">{cell("cacheRead")}</td>
                        <td className="py-1.5 pr-2 text-right">{cell("cacheWrite")}</td>
                        <td className="py-1.5 pr-2 text-right">
                          <input
                            type="number"
                            step="0.01"
                            min="0"
                            value={p?.peak ? String(p.peak.input) : ""}
                            placeholder={p?.peak ? "" : "—"}
                            title="高峰单价(输入);留空 = 无峰谷"
                            onChange={(e) => updatePeak(model, e.target.value)}
                            onBlur={persistNow}
                            data-theme-part="input"
                            className="h-7 w-20 rounded-lg border border-border bg-background px-2 text-right tabular-nums outline-none focus:border-primary"
                          />
                        </td>
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
        icon={<GaugeIcon className="h-4 w-4 text-primary" />}
        title="额度来源"
        collapsible
        summary={
          settings
            ? (settings.limitProviders ?? DEFAULT_LIMIT_PROVIDERS)
                .map((p) => LIMIT_PROVIDER_LABELS[p] ?? p)
                .join("、") || "全部关闭"
            : undefined
        }
      >
        <LimitSources onChanged={onDataChanged} />
      </SectionCard>

      <SectionCard
        icon={<RefreshIcon className="h-4 w-4 text-primary" />}
        title="操作"
      >
        <div className="flex flex-wrap items-center gap-3 p-4">
          <button
            type="button"
            onClick={handleRefresh}
            disabled={busyAction !== "none"}
            data-theme-part="button"
            className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-xs font-medium transition-colors hover:bg-overlay/5 disabled:opacity-50"
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
        title="外观与启动"
      >
        <div className="px-4 py-3">
          <div className="flex items-center justify-between gap-3">
            <div className="min-w-0">
              <div className="text-sm font-medium">主题</div>
              <div className="mt-0.5 truncate text-xs text-muted-foreground">
                {themeCtx.fallbackReason ?? `当前:${themeCtx.theme.name}`}
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <SelectMenu
                value={selectedExists ? themeCtx.selectedId : "__missing__"}
                onChange={applyThemeId}
                // 设置快照还没读到时不能选:选了会生效却存不进设置文件
                disabled={!settings}
                title="选择主题;第三方主题放进下方目录后点「重新扫描」"
                ariaLabel="主题"
                className="h-8 max-w-[220px]"
                options={[
                  ...(selectedExists
                    ? []
                    : [
                        {
                          value: "__missing__",
                          label: `${themeCtx.selectedId}(未找到)`,
                          disabled: true,
                        },
                      ]),
                  ...themeCtx.entries.map((e) => ({
                    value: e.manifest ? e.id : `__invalid__:${e.path ?? e.id}`,
                    label: `${e.name}${e.source === "builtin" ? " · 内置" : ""}${
                      e.manifest ? "" : "(无效)"
                    }`,
                    disabled: !e.manifest,
                  })),
                ]}
              />
              <button
                type="button"
                onClick={() => void reloadThemes()}
                disabled={themeReloading}
                title="重新扫描主题目录"
                data-theme-part="button"
                className="inline-flex h-8 items-center gap-1 rounded-lg border border-border bg-background px-2.5 text-xs font-medium transition-colors hover:bg-overlay/5 disabled:opacity-50"
              >
                <RefreshIcon
                  className={`h-3.5 w-3.5 ${themeReloading ? "animate-spin" : ""}`}
                />
                重新扫描
              </button>
            </div>
          </div>
          <p className="mt-2 break-all text-11px text-muted-foreground">
            第三方主题:把 <code className="font-mono">theme.json</code> 放进{" "}
            <code className="font-mono">{themeCtx.themesDir ?? "(应用数据目录)/themes"}</code>
            {" "}后重新扫描;格式见 docs/theme_interface.md
          </p>
          {themeCtx.listingError ? (
            <p className="mt-1 text-11px text-danger-text">
              主题目录读取失败:{themeCtx.listingError}
            </p>
          ) : null}
          {themeIssues.length > 0 ? (
            <ul className="mt-2 space-y-1 text-11px">
              {themeIssues.map((e) => (
                <li key={e.path ?? e.id}>
                  <span className="font-medium">{e.name}</span>
                  <span className="text-muted-foreground">({e.path})</span>
                  <ul className="mt-0.5 space-y-0.5 pl-3">
                    {e.diagnostics.slice(0, 8).map((d, i) => (
                      <li
                        key={i}
                        className={
                          d.level === "error" ? "text-danger-text" : "text-warning-text"
                        }
                      >
                        {d.level === "error" ? "错误" : "警告"}
                        {d.path ? ` · ${d.path}` : ""}:{d.message}
                      </li>
                    ))}
                    {e.diagnostics.length > 8 ? (
                      <li className="text-muted-foreground">
                        …还有 {e.diagnostics.length - 8} 条
                      </li>
                    ) : null}
                  </ul>
                </li>
              ))}
            </ul>
          ) : null}
        </div>

        <div className="flex items-center justify-between border-t border-border/40 px-4 py-3">
          <div>
            <div className="text-sm font-medium">深浅模式</div>
            <div className="mt-0.5 text-xs text-muted-foreground">
              当前:{modeLabel(theme)}
              {singleMode
                ? themeCtx.preferredMode !== singleMode
                  ? `(该主题只提供${modeLabel(singleMode)};你偏好的${modeLabel(themeCtx.preferredMode)}会在换回双模式主题时恢复)`
                  : `(该主题只提供${modeLabel(singleMode)})`
                : ""}
            </div>
          </div>
          <div data-theme-part="segmented" className="flex items-center gap-1 rounded-xl bg-muted p-1">
            <button
              type="button"
              onClick={() => applyTheme("dark")}
              disabled={singleMode !== null || !settings}
              title={modeLockedTip}
              data-theme-part="segmented-button"
              data-theme-state={theme === "dark" ? "selected" : undefined}
              className={`flex h-7 items-center gap-1 rounded-lg px-2.5 text-xs font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
                theme === "dark"
                  ? "bg-background shadow-sm text-foreground"
                  : "text-muted-foreground hover:bg-overlay/5"
              }`}
            >
              <MoonIcon className="h-3.5 w-3.5" />
              暗色
            </button>
            <button
              type="button"
              onClick={() => applyTheme("light")}
              disabled={singleMode !== null || !settings}
              title={modeLockedTip}
              data-theme-part="segmented-button"
              data-theme-state={theme === "light" ? "selected" : undefined}
              className={`flex h-7 items-center gap-1 rounded-lg px-2.5 text-xs font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
                theme === "light"
                  ? "bg-background shadow-sm text-foreground"
                  : "text-muted-foreground hover:bg-overlay/5"
              }`}
            >
              <SunIcon className="h-3.5 w-3.5" />
              亮色
            </button>
          </div>
        </div>

        <div className="flex items-center justify-between border-t border-border/40 px-4 py-3">
          <div className="text-sm font-medium">启动时最小化到托盘</div>
          {/* 与其它开关同一个组件(以前是一套 primary 轨道 + background 滑块的小号实现) */}
          <Toggle
            checked={settings?.startMinimized ?? false}
            onChange={() =>
              settings &&
              void persist({
                ...settings,
                startMinimized: !settings.startMinimized,
              })
            }
            ariaLabel="启动时最小化到托盘"
          />
        </div>
      </SectionCard>
    </div>
  );
}
