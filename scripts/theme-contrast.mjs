#!/usr/bin/env node
/**
 * 主题对比度检查(给主题作者用,可选)。
 *
 *   node scripts/theme-contrast.mjs path/to/theme.json [更多文件...]
 *
 * 先用应用里同一份代码(src/theme/)校验并解析主题,再按界面里**真实的**前景/背景
 * 组合计算 WCAG 2.x 对比度。组合来自组件里的用法(见下方 PAIRS 的注释),
 * 半透明底色(如 `bg-success/10`)按浏览器的方式在 sRGB 里与下层混合后再算。
 *
 * 阈值:文字 4.5:1(WCAG AA 正文);图形/控件 3:1(WCAG 1.4.11)。
 * 标「参考」的项只打印、不计入结果(组件写法决定的组合,不是每个主题都能兼顾,见注释)。
 * 输出每个模式的检查表;有低于阈值的(非参考)项时退出码为 1。
 * 依赖 esbuild(vite 自带),先 `npm install`。
 */
import fs from "node:fs";
import { loadThemeModule } from "./lib/load-theme.mjs";

const files = process.argv.slice(2);
if (files.length === 0) {
  console.error("用法:node scripts/theme-contrast.mjs <theme.json> [...]");
  process.exit(2);
}

// 与 theme-lint.mjs 相同:把 src/theme 打成临时 ESM 模块再导入
const theme = await loadThemeModule("theme-contrast");

const { parseThemeFile, resolveTheme, parseColor, BUILTIN_THEMES } = theme;
const reserved = BUILTIN_THEMES.map((t) => t.id);

const TEXT = 4.5;
const UI = 3;
/** 参考项:按 3:1 打印,不计入失败 */
const REF = "ref";

function luminance({ r, g, b }) {
  const lin = (v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

function ratio(a, b) {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** `top` 以 `alpha` 盖在 `under` 上(sRGB 混合,与浏览器一致) */
function over(top, alpha, under) {
  const mix = (k) => top[k] * alpha + under[k] * (1 - alpha);
  return { r: mix("r"), g: mix("g"), b: mix("b"), a: 1 };
}

/**
 * 需要检查的组合:[说明, 前景, 背景, 阈值(TEXT / UI / REF)]。
 * 前景/背景是函数,参数 c(token 名 → 颜色)。
 */
const PAIRS = [
  // 正文与次要文字
  ["正文 foreground / background", (c) => c.foreground, (c) => c.background, TEXT],
  ["正文 foreground / card(卡片文字、图表 tooltip)", (c) => c.foreground, (c) => c.card, TEXT],
  ["次要文字 mutedForeground / background", (c) => c.mutedForeground, (c) => c.background, TEXT],
  ["次要文字 mutedForeground / card(刻度、说明)", (c) => c.mutedForeground, (c) => c.card, TEXT],
  ["次要文字 mutedForeground / muted(分段控件、灰角标)", (c) => c.mutedForeground, (c) => c.muted, TEXT],
  ["选中 chip:foreground / primary 15% 叠在 background", (c) => c.foreground, (c) => over(c.primary, 0.15, c.background), TEXT],
  // 主色
  ["链接/角标 primary / card", (c) => c.primary, (c) => c.card, TEXT],
  ["角标 primary / primary 10% 叠在 card", (c) => c.primary, (c) => over(c.primary, 0.1, c.card), TEXT],
  ["按钮 primaryForeground / primary", (c) => c.primaryForeground, (c) => c.primary, TEXT],
  // 危险 / 状态文字
  ["错误文字 destructive / card", (c) => c.destructive, (c) => c.card, TEXT],
  ["重扫按钮 destructive / background", (c) => c.destructive, (c) => c.background, TEXT],
  ["dangerText / card(错误提示)", (c) => c.dangerText, (c) => c.card, TEXT],
  ["successText / success 10% 叠在 card(绿角标)", (c) => c.successText, (c) => over(c.success, 0.1, c.card), TEXT],
  ["warningText / warning 15% 叠在 card(警告角标)", (c) => c.warningText, (c) => over(c.warning, 0.15, c.card), TEXT],
  ["warningText / card(警告说明)", (c) => c.warningText, (c) => c.card, TEXT],
  // 状态「填充色」也被当文字用:额度剩余百分比、缓存命中率标题、保存成功提示
  ["success 当文字 / card", (c) => c.success, (c) => c.card, TEXT],
  ["warning 当文字 / card", (c) => c.warning, (c) => c.card, TEXT],
  ["notice / notice 15% 叠在 card(新版本角标)", (c) => c.notice, (c) => over(c.notice, 0.15, c.card), TEXT],
  // 统计卡六项指标:11px 标签,底色是 background 40% 叠在 card
  ...["input", "output", "cacheRead", "cacheWrite", "calls", "cost"].map((k) => [
    `统计标签 stat.${k} / MiniStat 底`,
    (c) => c[`stat.${k}`],
    (c) => over(c.background, 0.4, c.card),
    TEXT,
  ]),
  // 图形 / 控件(3:1)
  ["焦点环 ring / background", (c) => c.ring, (c) => c.background, UI],
  ...["success", "warning", "danger"].map((k) => [
    `进度条 ${k} / 轨道 muted 60% 叠在 card`,
    (c) => c[k],
    (c) => over(c.muted, 0.6, c.card),
    UI,
  ]),
  // 设置页开关的滑块用 primaryForeground(轨道:开 = success,关 = mutedForeground 30% 叠在 card)。
  // 主色很亮、primaryForeground 取深色的暗色主题,「关」态滑块必然很暗;可用 shadow.base
  // (只用于开关滑块)给滑块描一圈浅色边来补救,这里算不到阴影,所以只作参考。
  ["开关滑块 primaryForeground / 开 success", (c) => c.primaryForeground, (c) => c.success, REF],
  ["开关滑块 primaryForeground / 关 mutedForeground 30% 叠在 card", (c) => c.primaryForeground, (c) => over(c.mutedForeground, 0.3, c.card), REF],
];

/** 主题的 cssVars → { token 名: 颜色 } */
function colorsOf(r) {
  const out = {};
  const camel = (s) => s.replace(/-([a-z])/g, (_, x) => x.toUpperCase());
  for (const [k, v] of Object.entries(r.cssVars)) {
    const c = parseColor(v);
    if (!c) continue;
    if (k.startsWith("--stat-")) out[`stat.${camel(k.slice(7))}`] = c;
    else out[camel(k.slice(2))] = c;
  }
  return out;
}

let failed = false;
for (const file of files) {
  console.log(`\n== ${file}`);
  let text;
  try {
    text = fs.readFileSync(file, "utf8");
  } catch (err) {
    console.log(`  读取失败:${err.message}`);
    failed = true;
    continue;
  }
  const { manifest } = parseThemeFile(text, { reservedIds: reserved });
  if (!manifest) {
    console.log("  ✗ 校验未通过,先用 scripts/theme-lint.mjs 修正");
    failed = true;
    continue;
  }
  for (const mode of Object.keys(manifest.modes)) {
    const r = resolveTheme(manifest, mode, "user");
    const c = colorsOf(r);
    let worstText = Infinity;
    let fails = 0;
    console.log(`  · ${mode}`);
    const check = (label, fg, bg, min) => {
      const v = ratio(fg, bg);
      if (min === REF) {
        console.log(`    · ${v.toFixed(2).padStart(5)}:1 (参考) ${label}`);
        return;
      }
      const ok = v >= min;
      if (!ok) fails++;
      if (min === TEXT) worstText = Math.min(worstText, v);
      console.log(`    ${ok ? "✓" : "✗"} ${v.toFixed(2).padStart(5)}:1 (≥${min}) ${label}`);
    };
    for (const [label, fg, bg, min] of PAIRS) check(label, fg(c), bg(c), min);
    // 图表:调色板与 Agent 品牌色画在卡片上(线、扇区、图标),按图形 3:1
    r.chart.palette.forEach((hex, i) =>
      check(`图表 palette[${i}] ${hex} / card`, parseColor(hex), c.card, UI),
    );
    for (const [id, hex] of Object.entries(r.chart.agents)) {
      check(`Agent ${id} ${hex} / card`, parseColor(hex), c.card, UI);
    }
    console.log(
      `    → 文字最低 ${worstText.toFixed(2)}:1;${fails === 0 ? "全部达标" : `${fails} 项低于阈值`}`,
    );
    if (fails > 0) failed = true;
  }
}
process.exit(failed ? 1 : 0);
