import { invoke } from "@tauri-apps/api/core";

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

export interface UsageSummary {
  generatedAt: number;
  today: Totals;
  week: Totals;
  month: Totals;
  allTime: Totals;
  byAgentToday: AgentSlice[];
  byModelMonth: ModelSlice[];
}

/** 任意日期范围(可按 Agent 过滤)的统计;日期格式 "YYYY-MM-DD"(本地) */
export interface RangeSummary {
  generatedAt: number;
  from: string;
  to: string;
  agent: string | null;
  totals: Totals;
  byAgent: AgentSlice[];
  byModel: ModelSlice[];
}

export interface AgentStatus {
  id: string;
  displayName: string;
  detected: boolean;
  enabled: boolean;
  todayTokens: number;
  todayCost: number;
  totalTokens: number;
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

/** 模型定价($ / 百万 tokens);用于给无自带成本的数据估算费用 */
export interface PriceEntry {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

export interface Settings {
  enabledAgents: string[];
  startMinimized: boolean;
  theme: string;
  customAgents: CustomAgentConfig[];
  pricing: Record<string, PriceEntry>;
  /** 美元 → 人民币汇率(估算换算) */
  exchangeRate: number;
}

export const AGENT_LABELS: Record<string, string> = {
  dsh: "DSH",
  "claude-code": "Claude Code",
  codex: "Codex CLI",
  zcode: "ZCode",
  opencode: "OpenCode",
  pi: "Pi",
};

export const AGENT_COLORS: Record<string, string> = {
  dsh: "#8b5cf6",
  "claude-code": "#f59e0b",
  codex: "#3b82f6",
  zcode: "#10b981",
  opencode: "#06b6d4",
  pi: "#ec4899",
};

const FALLBACK_PALETTE = [
  "#ec4899",
  "#f97316",
  "#84cc16",
  "#a855f7",
  "#14b8a6",
  "#eab308",
];

/** 未知 Agent(自定义等)的稳定配色:按 id 哈希取色 */
export function agentColor(id: string): string {
  if (AGENT_COLORS[id]) return AGENT_COLORS[id];
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return FALLBACK_PALETTE[h % FALLBACK_PALETTE.length];
}

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
};
