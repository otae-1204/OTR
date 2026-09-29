/**
 * 内置主题。
 *
 * `otr` 是应用的默认外观,也是所有缺失 token 的最终回退来源,
 * 因此它必须是**完整的**:每个 token 在两种模式下都有值。
 * 这里的数值与 src/index.css 里 `:root` / `.dark` 的 CSS 变量逐位一致
 * (index.css 那份是 JS 跑起来之前的兜底,改一处务必同步另一处)。
 */

import type { ThemeManifest, ThemeTokens, ThemeMode } from "./types";
import { validateManifest } from "./validate";

/** 与深浅模式无关的公共 token */
const OTR_COMMON: ThemeTokens = {
  stat: {
    input: "#3b82f6", // blue-500
    output: "#a855f7", // purple-500
    cacheRead: "#10b981", // emerald-500
    cacheWrite: "#f59e0b", // amber-500
    calls: "#0ea5e9", // sky-500
    cost: "#22c55e", // green-500
  },
  chart: {
    palette: [
      "#3b82f6",
      "#a855f7",
      "#10b981",
      "#f97316",
      "#f59e0b",
      "#06b6d4",
      "#ec4899",
      "#84cc16",
      "#64748b",
    ],
    agents: {
      dsh: "#8b5cf6",
      "claude-code": "#f59e0b",
      codex: "#3b82f6",
      zcode: "#10b981",
      opencode: "#06b6d4",
      pi: "#ec4899",
      cursor: "#A3A3A3",
    },
    agentFallback: [
      "#ec4899",
      "#f97316",
      "#84cc16",
      "#a855f7",
      "#14b8a6",
      "#eab308",
    ],
  },
  font: {
    sans: '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "PingFang SC", "Microsoft YaHei", sans-serif',
    mono: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace',
  },
  radius: {
    md: "0.375rem",
    lg: "0.75rem",
    xl: "0.875rem",
  },
  shadow: {
    sm: "0 1px 2px 0 rgb(0 0 0 / 0.05)",
    base: "0 1px 3px 0 rgb(0 0 0 / 0.1), 0 1px 2px -1px rgb(0 0 0 / 0.1)",
    md: "0 4px 6px -1px rgb(0 0 0 / 0.1), 0 2px 4px -2px rgb(0 0 0 / 0.1)",
    lg: "0 10px 15px -3px rgb(0 0 0 / 0.1), 0 4px 6px -4px rgb(0 0 0 / 0.1)",
  },
};

const OTR_LIGHT: ThemeTokens = {
  colors: {
    background: "0 0% 100%",
    foreground: "240 6% 10%",
    card: "0 0% 100%",
    cardForeground: "240 6% 10%",
    popover: "0 0% 100%",
    popoverForeground: "240 6% 10%",
    primary: "210 100% 56%",
    primaryForeground: "0 0% 100%",
    secondary: "240 5% 96%",
    secondaryForeground: "240 6% 10%",
    muted: "240 5% 96%",
    mutedForeground: "240 4% 46%",
    accent: "240 5% 96%",
    accentForeground: "240 6% 10%",
    destructive: "0 72% 51%",
    destructiveForeground: "0 0% 100%",
    border: "240 5.9% 90%",
    input: "240 5.9% 90%",
    ring: "210 100% 56%",
    overlay: "0 0% 0%",
    success: "#10b981", // emerald-500
    warning: "#f59e0b", // amber-500
    danger: "#ef4444", // red-500
    info: "#0ea5e9", // sky-500
    notice: "#f97316", // orange-500
    successText: "#059669", // emerald-600
    warningText: "#d97706", // amber-600
    dangerText: "#ef4444", // red-500
    // 独立状态文字:改造前这几处用的是填充色(emerald/amber-500,白底上只有 2.2–2.5:1),
    // 现在各深一到两档,白色卡片上 ≥ 4.5:1
    successLabel: "#047857", // emerald-700
    warningLabel: "#b45309", // amber-700
    dangerLabel: "#dc2626", // red-600
    // 开关滑块:开 = 白(与改造前相同);关 = zinc-500,浅灰轨道上 3.3:1(改造前白色只有 1.5:1)
    switchThumb: "#ffffff",
    switchThumbOff: "#71717a", // zinc-500
  },
};

const OTR_DARK: ThemeTokens = {
  colors: {
    background: "240 5% 12%",
    foreground: "0 0% 93%",
    card: "240 5% 16%",
    cardForeground: "0 0% 93%",
    popover: "240 5% 14%",
    popoverForeground: "0 0% 93%",
    primary: "210 100% 56%",
    primaryForeground: "0 0% 100%",
    secondary: "240 5% 20%",
    secondaryForeground: "0 0% 93%",
    muted: "240 5% 20%",
    mutedForeground: "240 5% 65%",
    accent: "240 5% 20%",
    accentForeground: "0 0% 93%",
    destructive: "0 62% 45%",
    destructiveForeground: "0 0% 100%",
    border: "240 5% 24%",
    input: "240 5% 24%",
    ring: "210 100% 56%",
    overlay: "0 0% 100%",
    success: "#10b981",
    warning: "#f59e0b",
    danger: "#ef4444",
    info: "#0ea5e9",
    notice: "#f97316",
    successText: "#34d399", // emerald-400
    warningText: "#fbbf24", // amber-400
    dangerText: "#ef4444",
    // 暗色卡片上 emerald/amber-500 本来就够(5.9 / 7.0:1),保持不变;红色改浅一档(3.97 → 5.4:1)
    successLabel: "#10b981", // emerald-500
    warningLabel: "#f59e0b", // amber-500
    dangerLabel: "#f87171", // red-400
    switchThumb: "#ffffff",
    switchThumbOff: "#ffffff",
  },
};

/** 默认主题的原始清单(与用户主题文件同一格式) */
const OTR_RAW: ThemeManifest = {
  apiVersion: 1,
  id: "otr",
  name: "OTR 默认",
  version: "1.0.0",
  author: "OTR",
  description: "应用自带的默认外观,深浅两种模式。",
  tokens: OTR_COMMON,
  modes: {
    dark: OTR_DARK,
    light: OTR_LIGHT,
  },
};

/**
 * 内置主题走与用户主题**完全相同**的校验/归一化路径:
 * 十六进制会被转成 HSL 三元组、图表色转成 #rrggbb。这样解析器只需要处理一种形态,
 * 也顺便保证内置清单本身永远是合法的示例。
 */
function normalizeBuiltin(raw: ThemeManifest): ThemeManifest {
  const { manifest, diagnostics } = validateManifest(raw);
  const problems = diagnostics.filter((d) => d.level === "error");
  if (!manifest || problems.length > 0) {
    // 内置主题写坏了是开发期错误,直接抛出让构建/首屏立刻暴露
    throw new Error(
      `内置主题「${raw.id}」不合法:${problems.map((d) => `${d.path}: ${d.message}`).join("; ")}`,
    );
  }
  for (const d of diagnostics) {
    console.warn(`[theme] 内置主题「${raw.id}」${d.path}: ${d.message}`);
  }
  return manifest;
}

/** 默认主题:当前应用外观的完整描述(已归一化) */
export const OTR_THEME: ThemeManifest = normalizeBuiltin(OTR_RAW);

/** 随应用打包的主题,按展示顺序排列;第一个必须是默认主题 */
export const BUILTIN_THEMES: readonly ThemeManifest[] = [OTR_THEME];

export function builtinModeTokens(mode: ThemeMode): ThemeTokens {
  return OTR_THEME.modes[mode] ?? OTR_DARK;
}
