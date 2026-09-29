import { invoke } from "@tauri-apps/api/core";
import type { ThemeFile } from "../theme/types";

export interface Totals {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  calls: number;
  totalTokens: number;
  cost: number;
}

export interface AgentSlice {
  agent: string;
  totals: Totals;
}

export interface ModelSlice {
  model: string;
  totals: Totals;
}

/** 轻量摘要:只有今日各 Agent 卡片 + 数据版本号(明细走 RangeSummary / DailyUsage / SessionUsage) */
export interface UsageSummary {
  /** 数据最后一次真正变化的时间,不是查询时间 */
  generatedAt: number;
  /** 只在真的写入用量时递增;前端用它当刷新键 */
  dataVersion: number;
  byAgentToday: AgentSlice[];
}

/** 任意日期范围(可按 Agent 过滤)的统计;日期格式 "YYYY-MM-DD"(本地) */
export interface RangeSummary {
  generatedAt: number;
  from: string;
  to: string;
  agent: string | null;
  /** 本结果成本的币种(跟随全局设置,当前恒为归一化后的 "CNY") */
  currency: string;
  totals: Totals;
  byAgent: AgentSlice[];
  byModel: ModelSlice[];
}

export interface AgentStatus {
  id: string;
  displayName: string;
  detected: boolean;
  enabled: boolean;
  /** 全部时间累计 tokens(卡片右侧那个"累计") */
  totalTokens: number;
  /** 需要用户注意的健康提示(登录失效/分页截断…);null = 正常 */
  notice: string | null;
}

export interface DailyUsage {
  date: string;
  agent: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  calls: number;
  totalTokens: number;
  cost: number;
}

export interface SessionUsage {
  agent: string;
  sessionId: string | null;
  project: string | null;
  title: string | null;
  models: string | null;
  startedAt: number | null;
  lastActive: number | null;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  calls: number;
  totalTokens: number;
  cost: number;
}

/** 自定义 Agent:复用内置解析器,指向用户指定的数据目录 */
export interface CustomAgentConfig {
  id: string;
  name: string;
  kind: "claude-code" | "codex" | "zcode";
  dir: string;
}

/** 高峰档定价($ / 百万 tokens);缺省 = 该模型无峰谷,按平价计 */
export interface PeakTier {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

/** 模型定价($ / 百万 tokens);用于给无自带成本的数据估算费用 */
export interface PriceEntry {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  /** DeepSeek 等有峰谷分时计价的模型;缺省时行为与加峰谷之前完全一致 */
  peak?: PeakTier | null;
}

// ---- 额度页 ----

/** 一个额度窗口(5 小时 / 每周 / 每月 / 套餐…) */
export interface QuotaWindow {
  key: string;
  label: string;
  /** 已用百分比;null = 上游没给这个字段 → 显示 "--",**不是** 0 */
  usedPercent?: number | null;
  /** 重置时刻(unix ms) */
  resetAt?: number | null;
  windowSeconds?: number | null;
}

export interface Balance {
  amount: number;
  currency: string;
  cash?: number | null;
  voucher?: number | null;
}

/** 一个账号的额度快照 */
export interface ProviderLimits {
  accountId: string;
  accountLabel: string;
  provider: string;
  /** false = 还没配凭据,显示引导文案而不是红色报错 */
  configured: boolean;
  error?: string | null;
  planLabel?: string | null;
  windows: QuotaWindow[];
  balance?: Balance | null;
  /** 本次抓取时刻;0 = 从未成功 */
  fetchedAt: number;
}

export interface LimitAccount {
  id: string;
  provider: string;
  label: string;
  plan: string;
  /** home 模式:该账号的 profile 目录 */
  home?: string | null;
  /** key 模式:系统凭据库里的 API Key 条目名。界面不展示。 */
  secretRef?: string | null;
  /** StepFun 控制台 Cookie 的条目名。界面不展示。 */
  cookieRef?: string | null;
  /** API Key 是否已写入。列表接口才有,保存时不要带。 */
  keyPresent?: boolean;
  /** 控制台 Cookie 是否已写入。 */
  cookiePresent?: boolean;
  /** 内置账号不可删 */
  builtin: boolean;
}

export interface CredentialView {
  name: string;
  present: boolean;
}

/** 支持的额度来源及其 UI 文案 */
export const LIMIT_PROVIDER_LABELS: Record<string, string> = {
  cursor: "Cursor",
  codex: "Codex CLI",
  deepseek: "DeepSeek",
  qwen: "Qwen",
  stepfun: "StepFun",
};

/** home 模式(认证靠本地 CLI 会话)vs key 模式(额度 API 直接吃 key) */
export const LIMIT_PROVIDER_MODE: Record<string, "home" | "key"> = {
  cursor: "home",
  codex: "home",
  deepseek: "key",
  qwen: "key",
  stepfun: "key",
};

/** 已实测可用的来源;StepFun 是逆向接口,默认关闭 */
export const DEFAULT_LIMIT_PROVIDERS = ["cursor", "codex", "deepseek", "qwen"];

export interface Settings {
  enabledAgents: string[];
  startMinimized: boolean;
  /**
   * 生效的深浅模式:"dark" | "light"(历史字段名,含义是模式而不是主题)。
   * 选中单模式主题时它跟着变;用户的选择记在 preferredMode。
   */
  theme: string;
  /**
   * 用户偏好的深浅模式:"dark" | "light"。只由设置页的深浅按钮修改,单模式主题不会改它;
   * 换回支持该模式的主题时按它恢复。旧设置文件缺这个字段时由 theme 推导。
   */
  preferredMode?: string;
  /** 当前主题 id(内置 "otr" 或用户主题目录里的主题);缺省 = "otr" */
  themeId?: string;
  customAgents: CustomAgentConfig[];
  pricing: Record<string, PriceEntry>;
  /** 定价来源:model -> "manual" | "models.dev:<provider>";仅用于 UI 展示 */
  pricingSource: Record<string, string>;
  /** 美元 → 人民币汇率(估算换算) */
  exchangeRate: number;
  /** 全局成本显示币种:"CNY" | "USD" */
  currency: string;
  /** 额度后台刷新间隔(秒) */
  refreshSecs?: number;
  /** 额度页的额外账号(内置账号由后端合成,不在这里) */
  limitAccounts?: LimitAccount[];
  /** 显式启用过的额度来源 */
  limitProviders?: string[];
}

export const AGENT_LABELS: Record<string, string> = {
  dsh: "DSH",
  "claude-code": "Claude Code",
  codex: "Codex CLI",
  zcode: "ZCode",
  opencode: "OpenCode",
  pi: "Pi",
  cursor: "Cursor",
};

/*
 * Agent 品牌色与图表调色板现在由主题提供(src/theme/),
 * 组件通过 useTheme().agentColor(id) 取色;默认值见 src/theme/builtin.ts。
 */

export const api = {
  listAgents: () => invoke<AgentStatus[]>("list_agents"),
  getSummary: () => invoke<UsageSummary>("get_summary"),
  getRangeSummary: (agent: string | null, from: string, to: string) =>
    invoke<RangeSummary>("get_range_summary", { agent, from, to }),
  getDaily: (agent: string | null, from: string, to: string, granularity?: "day" | "hour" | "month") =>
    invoke<DailyUsage[]>("get_daily", { agent, from, to, granularity: granularity ?? "day" }),
  getSessions: (
    agent: string | null,
    from: string | null,
    to: string | null,
    limit?: number,
  ) =>
    invoke<SessionUsage[]>("get_sessions", {
      agent,
      from,
      to,
      limit: limit ?? 100,
    }),
  listModels: () => invoke<string[]>("list_models"),
  rescan: (full?: boolean) => invoke<void>("rescan", { full: full ?? false }),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) =>
    invoke<void>("save_settings", { settings }),
  // 额度页:getLimits 读内存缓存(不发网络),refreshLimits 才真的拉
  getLimits: () => invoke<ProviderLimits[]>("get_limits"),
  refreshLimits: (account?: string | null) =>
    invoke<ProviderLimits[]>("refresh_limits", { account: account ?? null }),
  listLimitAccounts: () => invoke<LimitAccount[]>("list_limit_accounts"),
  saveLimitAccount: (
    account: Omit<LimitAccount, "builtin" | "keyPresent" | "cookiePresent">,
    secrets?: { apiKey?: string | null; consoleCookie?: string | null },
  ) =>
    invoke<void>("save_limit_account", {
      account,
      apiKey: secrets?.apiKey ?? null,
      consoleCookie: secrets?.consoleCookie ?? null,
    }),
  deleteLimitAccount: (id: string) =>
    invoke<void>("delete_limit_account", { id }),
  setLimitProvider: (provider: string, enabled: boolean) =>
    invoke<void>("set_limit_provider", { provider, enabled }),
  listLimitCredentials: () => invoke<CredentialView[]>("list_limit_credentials"),
  saveLimitCredential: (name: string, secret: string) =>
    invoke<void>("save_limit_credential", { name, secret }),
  deleteLimitCredential: (name: string) =>
    invoke<void>("delete_limit_credential", { name }),
  /** 打开 Cookie 教程窗口。只在用户点击时调用。 */
  openCookieGuide: (provider: string) =>
    invoke<void>("open_cookie_guide", { provider }),
  // 主题:Rust 只负责枚举并读出用户主题目录里的文件,解析与校验在前端 src/theme/
  listThemes: () => invoke<ThemeFile[]>("list_themes"),
  getThemesDir: () => invoke<string>("get_themes_dir"),
};
