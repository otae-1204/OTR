import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  api,
  DEFAULT_LIMIT_PROVIDERS,
  type LimitAccount,
} from "../../api/bindings";
import type { ConfirmOptions } from "../ConfirmDialog";
import { PlusIcon, TrashIcon } from "../icons";
import { Toggle } from "./SettingsUI";

const PROVIDER_LABELS: Record<string, string> = {
  cursor: "Cursor",
  codex: "Codex CLI",
  grok: "Grok",
  antigravity: "Antigravity",
  deepseek: "DeepSeek",
  openrouter: "OpenRouter",
  minimax: "MiniMax",
  zai: "智谱",
  kimi: "Kimi",
  factory: "Factory",
  mistral: "Mistral",
  qwen: "Qwen",
  stepfun: "StepFun",
};

/** home 模式靠本地 CLI 会话(要目录),key 模式靠 API key(表单里直接粘贴) */
const PROVIDER_MODE: Record<string, "home" | "key"> = {
  cursor: "home",
  codex: "home",
  grok: "home",
  antigravity: "home",
  deepseek: "key",
  openrouter: "key",
  minimax: "key",
  zai: "key",
  kimi: "key",
  factory: "key",
  mistral: "key",
  qwen: "key",
  stepfun: "key",
};

const PROVIDER_ORDER = [
  "cursor",
  "codex",
  "grok",
  "antigravity",
  "deepseek",
  "openrouter",
  "minimax",
  "zai",
  "kimi",
  "factory",
  "mistral",
  "qwen",
  "stepfun",
] as const;

const BUILTIN_API_KEY: Record<string, { name: string; placeholder: string }> = {
  deepseek: { name: "DEEPSEEK_API_KEY", placeholder: "更新 API Key" },
  openrouter: { name: "OPENROUTER_API_KEY", placeholder: "更新 API Key" },
  minimax: { name: "MINIMAX_CODING_API_KEY", placeholder: "更新 API Key" },
  zai: { name: "ZAI_API_KEY", placeholder: "更新 API Key" },
  kimi: { name: "KIMI_CODE_API_KEY", placeholder: "更新 API Key" },
  factory: { name: "FACTORY_API_KEY", placeholder: "更新 API Key" },
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
  if (
    a.provider === "cursor" ||
    a.provider === "codex" ||
    a.provider === "grok" ||
    a.provider === "antigravity"
  ) {
    if (a.builtin) return "内置 · 本机会话";
    return a.home?.trim() || "未选择目录";
  }
  if (a.provider === "qwen") {
    const cookie = a.cookiePresent ? "Cookie 已配置" : "Cookie 未配置";
    const key = a.keyPresent ? "Key 已配置" : "Key 未配置";
    return a.builtin ? `内置 · ${cookie} · ${key}` : `${cookie} · ${key}`;
  }
  if (a.provider === "mistral") {
    const cookie = a.cookiePresent ? "Cookie 已配置" : "Cookie 未配置";
    return a.builtin ? `内置 · ${cookie}` : cookie;
  }
  if (a.provider === "stepfun") {
    const cookie = a.cookiePresent ? "订阅已配置" : "订阅未配置";
    const key = a.keyPresent ? "余额已配置" : "余额未配置";
    return a.builtin ? `内置 · ${cookie} · ${key}` : `${cookie} · ${key}`;
  }
  const key = a.keyPresent ? "API Key 已配置" : "API Key 未配置";
  return a.builtin ? `内置 · ${key}` : key;
}

const LIMIT_FOLD_KEY = "token-show-limit-source-fold";

function readLimitFolds(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(LIMIT_FOLD_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return {};
    const out: Record<string, boolean> = {};
    for (const [key, value] of Object.entries(parsed)) {
      if (value === true) out[key] = true;
    }
    return out;
  } catch {
    return {};
  }
}

function writeLimitFold(
  prev: Record<string, boolean>,
  provider: string,
  folded: boolean,
): Record<string, boolean> {
  const next = { ...prev, [provider]: folded };
  try {
    localStorage.setItem(LIMIT_FOLD_KEY, JSON.stringify(next));
  } catch {
    // 写不进去就只在这次打开里生效
  }
  return next;
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

/**
 * 额度来源:每个 Provider 一块。加同类账号只填显示名,再贴密钥或选目录。
 * id 和凭据条目名由程序生成,不出现在界面上。
 */
export function LimitSources({
  onChanged,
  confirm,
}: {
  onChanged: () => void;
  confirm: (options: ConfirmOptions) => Promise<boolean>;
}) {
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
  const [capturing, setCapturing] = useState<string | null>(null);
  const [folded, setFolded] = useState<Record<string, boolean>>(readLimitFolds);

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
      if (on) setFolded((prev) => writeLimitFold(prev, p, false));
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
    if (provider === "mistral") return draftCookie.trim().length > 0;
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
            provider === "stepfun" || provider === "qwen" || provider === "mistral"
              ? draftCookie.trim()
              : null,
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
    const ok = await confirm({
      title: `删除「${account.label}」？`,
      description: "已保存的密钥会一并清除，且无法撤销。",
      confirmLabel: "删除",
      tone: "destructive",
    });
    if (!ok) return;
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

  const captureCookie = async (provider: string) => {
    const builtin = (accounts ?? []).find(
      (a) => a.builtin && a.provider === provider && a.cookieRef,
    );
    if (!builtin?.cookieRef) {
      setErr("先打开这个额度来源,再自动获取");
      return;
    }
    setErr(null);
    setMsg(null);
    setCapturing(provider);
    try {
      const status = await api.captureConsoleCookie(provider, builtin.cookieRef);
      setMsg(status);
      reload();
      refreshLimits();
    } catch (e) {
      setErr(String(e));
    } finally {
      setCapturing(null);
    }
  };

  return (
    <div>
      <div className="divide-y divide-border/40">
        {PROVIDER_ORDER.map((p) => {
          const on = providers.includes(p);
          const rows = (accounts ?? []).filter((a) => a.provider === p);
          const expanded = on && !folded[p];
          return (
            <div key={p}>
              <div
                className={`flex items-center justify-between gap-3 px-4 py-3 ${
                  on ? "cursor-pointer hover:bg-overlay/5" : ""
                }`}
                onClick={
                  on
                    ? () => setFolded((prev) => writeLimitFold(prev, p, !prev[p]))
                    : undefined
                }
              >
                <div className="min-w-0 truncate text-sm font-medium">
                  {PROVIDER_LABELS[p]}
                </div>
                <div
                  className="flex shrink-0 items-center gap-2"
                  onClick={(e) => e.stopPropagation()}
                >
                  {p === "qwen" || p === "stepfun" || p === "mistral" ? (
                    <button
                      type="button"
                      onClick={() => openGuide(p)}
                      data-theme-part="button"
                      className="inline-flex h-6 shrink-0 items-center rounded-md border border-border bg-background px-2 text-11px font-medium transition-colors hover:border-primary/60 hover:text-primary"
                    >
                      教程
                    </button>
                  ) : null}
                  {p === "qwen" || p === "stepfun" || p === "mistral" ? (
                    <button
                      type="button"
                      disabled={capturing === p}
                      onClick={() => void captureCookie(p)}
                      data-theme-part="button"
                      className="inline-flex h-6 shrink-0 items-center rounded-md border border-border bg-background px-2 text-11px font-medium transition-colors hover:border-primary/60 hover:text-primary disabled:opacity-50"
                    >
                      {capturing === p ? "等待登录…" : "自动获取"}
                    </button>
                  ) : null}
                  <Toggle
                    checked={on}
                    onChange={() => void toggleProvider(p, !on)}
                    ariaLabel={`${on ? "关闭" : "开启"} ${PROVIDER_LABELS[p]}`}
                  />
                </div>
              </div>
              <div className={expanded ? "border-t border-border/40 bg-background/40 animate-fade-soft" : "hidden"}>
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
                        {a.builtin && a.provider === "mistral" ? (
                          <div className="space-y-2">
                            <BuiltinSecret
                              placeholder="更新 admin.mistral.ai 的 Cookie"
                              value={builtinDraft.MISTRAL_ADMIN_COOKIE ?? ""}
                              present={!!a.cookiePresent}
                              onChange={(value) =>
                                setBuiltinDraft((d) => ({
                                  ...d,
                                  MISTRAL_ADMIN_COOKIE: value,
                                }))
                              }
                              onSave={() => void saveBuiltin("MISTRAL_ADMIN_COOKIE")}
                              onClear={() => void clearBuiltin("MISTRAL_ADMIN_COOKIE")}
                            />
                            <p className="text-11px leading-relaxed text-muted-foreground">
                              点上方「自动获取」登录。窗口会在登录完成后自己关掉。也可以从请求头粘贴 Cookie。
                            </p>
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
                  <div className="space-y-2 border-t border-border/40 px-4 py-3 animate-fade-soft">
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
                    ) : p === "mistral" ? (
                      <>
                        <input
                          type="password"
                          autoComplete="off"
                          value={draftCookie}
                          onChange={(e) => setDraftCookie(e.target.value)}
                          placeholder="admin.mistral.ai 的 Cookie"
                          data-theme-part="input"
                          className={FIELD_CLASS}
                        />
                        <p className="text-11px leading-relaxed text-muted-foreground">
                          需要包含 ory_session_。过期后重新登录再贴，不能自动续期。
                        </p>
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
            </div>
          );
        })}
      </div>
      {msg ? <p className="px-4 py-2 text-xs text-success-label">{msg}</p> : null}
      {err ? <p className="px-4 py-2 text-xs text-danger-text">{err}</p> : null}
    </div>
  );
}
