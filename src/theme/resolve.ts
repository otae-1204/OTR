/**
 * 把「主题清单 + 模式」解析成一份完整的 ResolvedTheme。
 *
 * 叠加顺序(后者覆盖前者,按单个 token 粒度):
 *   1. 内置默认主题 `otr` 的公共 token + 该模式 token(完整,保证每个 token 都有值)
 *   2. 主题的公共 token(`tokens`)
 *   3. 主题的该模式 token(`modes[mode]`)
 *
 * `chart.palette` / `chart.agentFallback` 整组替换;`chart.agents` 按 Agent id 合并;
 * `css` 按「钩子[:状态] → 属性」粒度合并。
 */

import { OTR_THEME } from "./builtin";
import { kebab, mergeCss } from "./css";
import {
  COLOR_TOKENS,
  FONT_TOKENS,
  RADIUS_TOKENS,
  SHADOW_TOKENS,
  STAT_TOKENS,
  type ResolvedTheme,
  type ThemeManifest,
  type ThemeMode,
  type ThemeSource,
  type ThemeTokens,
} from "./types";

export { kebab };

function mergeTokens(base: ThemeTokens, over: ThemeTokens | undefined): ThemeTokens {
  if (!over) return base;
  return {
    colors: { ...base.colors, ...over.colors },
    stat: { ...base.stat, ...over.stat },
    chart: {
      palette: over.chart?.palette ?? base.chart?.palette,
      agents: { ...base.chart?.agents, ...over.chart?.agents },
      agentFallback: over.chart?.agentFallback ?? base.chart?.agentFallback,
    },
    font: { ...base.font, ...over.font },
    radius: { ...base.radius, ...over.radius },
    shadow: { ...base.shadow, ...over.shadow },
    css: mergeCss(base.css, over.css),
  };
}

/** 内置默认主题在某模式下的完整 token 集 */
export function defaultTokens(mode: ThemeMode): ThemeTokens {
  return mergeTokens(mergeTokens({}, OTR_THEME.tokens), OTR_THEME.modes[mode]);
}

/**
 * 解析。`mode` 不在主题支持的模式里也能解析:颜色来自默认主题该模式,
 * 公共 token 仍然生效。调用方应优先切到主题支持的模式(见 ThemeProvider)。
 */
export function resolveTheme(
  manifest: ThemeManifest,
  mode: ThemeMode,
  source: ThemeSource,
): ResolvedTheme {
  let t = defaultTokens(mode);
  t = mergeTokens(t, manifest.tokens);
  t = mergeTokens(t, manifest.modes[mode]);

  const cssVars: Record<string, string> = {};
  for (const k of COLOR_TOKENS) cssVars[`--${kebab(k)}`] = t.colors?.[k] ?? "";
  for (const k of STAT_TOKENS) cssVars[`--stat-${kebab(k)}`] = t.stat?.[k] ?? "";
  for (const k of FONT_TOKENS) cssVars[`--font-${k}`] = t.font?.[k] ?? "";
  for (const k of RADIUS_TOKENS) cssVars[`--radius-${k}`] = t.radius?.[k] ?? "";
  for (const k of SHADOW_TOKENS) {
    cssVars[k === "base" ? "--shadow" : `--shadow-${k}`] = t.shadow?.[k] ?? "";
  }
  const palette = t.chart?.palette ?? [];
  const agents = t.chart?.agents ?? {};
  const agentFallback = t.chart?.agentFallback ?? [];
  palette.forEach((c, i) => {
    cssVars[`--chart-${i + 1}`] = c;
  });
  for (const [id, c] of Object.entries(agents)) cssVars[`--agent-${id}`] = c;

  return {
    id: manifest.id,
    name: manifest.name,
    source,
    mode,
    cssVars,
    chart: { palette, agents, agentFallback },
    css: t.css ?? {},
  };
}

/** 稳定哈希取色:未配置品牌色的 Agent(自定义等)按 id 取一个固定颜色 */
export function pickAgentColor(theme: ResolvedTheme, id: string): string {
  const direct = theme.chart.agents[id];
  if (direct) return direct;
  const list = theme.chart.agentFallback;
  if (list.length === 0) return theme.chart.palette[0] ?? "#888888";
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return list[h % list.length];
}
