#!/usr/bin/env node
/**
 * 主题对比度检查(给主题作者用,可选)。
 *
 *   node scripts/theme-contrast.mjs path/to/theme.json [更多文件...]
 *   node scripts/theme-contrast.mjs --builtin                 # 检查内置默认主题 otr
 *   node scripts/theme-contrast.mjs --only label,switch a.json # 只看某几组
 *
 * 先用应用里同一份代码(src/theme/)校验并解析主题,再按界面里**真实的**前景/背景
 * 组合计算 WCAG 2.x 对比度。组合来自组件里的用法(见下方 PAIRS 的注释),
 * 半透明底色(如 `bg-success/10`)按浏览器的方式在 sRGB 里与下层混合后再算。
 *
 * 阈值:文字 4.5:1(WCAG AA 正文);图形/控件 3:1(WCAG 1.4.11)。
 * 分组(`--only` 用):text 正文与角标文字 / label 独立状态文字 / stat 统计标签 /
 * ui 焦点环与进度条 / switch 开关滑块(开、关两态)/ chart 图表与 Agent 品牌色。
 * 输出每个模式的检查表;有低于阈值的项时退出码为 1。
 * 依赖 esbuild(vite 自带),先 `npm install`。
 */
import fs from "node:fs";
import { loadThemeModule } from "./lib/load-theme.mjs";

const GROUPS = ["text", "label", "stat", "ui", "switch", "chart"];

const argv = process.argv.slice(2);
let builtin = false;
let only = null;
const files = [];
for (let i = 0; i < argv.length; i++) {
  const a = argv[i];
  if (a === "--builtin") builtin = true;
  else if (a === "--only") only = argv[++i] ?? "";
  else if (a.startsWith("--only=")) only = a.slice(7);
  else files.push(a);
}
const groups = only == null ? GROUPS : only.split(",").map((g) => g.trim()).filter(Boolean);
const badGroup = groups.find((g) => !GROUPS.includes(g));
if ((files.length === 0 && !builtin) || badGroup || groups.length === 0) {
  if (badGroup) console.error(`未知分组「${badGroup}」(可用:${GROUPS.join(" / ")})`);
  console.error("用法:node scripts/theme-contrast.mjs [--builtin] [--only 分组,…] [theme.json …]");
  process.exit(2);
}

// 与 theme-lint.mjs 相同:把 src/theme 打成临时 ESM 模块再导入
const theme = await loadThemeModule("theme-contrast");

const { parseThemeFile, resolveTheme, parseColor, BUILTIN_THEMES, OTR_THEME } = theme;
const reserved = BUILTIN_THEMES.map((t) => t.id);
function reservedFor(file) {
  const norm = file.replace(/\\/g, "/");
  const base = norm.split("/").pop()?.replace(/\.json$/, "");
  if (norm.includes("/examples/themes/")) {
    return reserved.filter((id) => id !== base);
  }
  return reserved;
}

const TEXT = 4.5;
const UI = 3;

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
 * 需要检查的组合:[分组, 说明, 前景, 背景, 阈值(TEXT / UI)]。
 * 前景/背景是函数,参数 c(token 名 → 颜色)。
 */
const PAIRS = [
  // 正文与次要文字
  ["text", "正文 foreground / background", (c) => c.foreground, (c) => c.background, TEXT],
  ["text", "正文 foreground / card(卡片文字、图表 tooltip)", (c) => c.foreground, (c) => c.card, TEXT],
  ["text", "次要文字 mutedForeground / background", (c) => c.mutedForeground, (c) => c.background, TEXT],
  ["text", "次要文字 mutedForeground / card(刻度、说明)", (c) => c.mutedForeground, (c) => c.card, TEXT],
  ["text", "次要文字 mutedForeground / muted(分段控件、灰角标)", (c) => c.mutedForeground, (c) => c.muted, TEXT],
  ["text", "选中 chip:foreground / primary 15% 叠在 background", (c) => c.foreground, (c) => over(c.primary, 0.15, c.background), TEXT],
  // 主色
  ["text", "链接/角标 primary / card", (c) => c.primary, (c) => c.card, TEXT],
  ["text", "角标 primary / primary 10% 叠在 card", (c) => c.primary, (c) => over(c.primary, 0.1, c.card), TEXT],
  ["text", "按钮 primaryForeground / primary", (c) => c.primaryForeground, (c) => c.primary, TEXT],
  // 危险 / 状态文字(角标与说明)
  ["text", "错误文字 destructive / card", (c) => c.destructive, (c) => c.card, TEXT],
  ["text", "重扫按钮 destructive / background", (c) => c.destructive, (c) => c.background, TEXT],
  ["text", "dangerText / card(错误提示)", (c) => c.dangerText, (c) => c.card, TEXT],
  ["text", "successText / success 10% 叠在 card(绿角标)", (c) => c.successText, (c) => over(c.success, 0.1, c.card), TEXT],
  ["text", "warningText / warning 15% 叠在 card(警告角标)", (c) => c.warningText, (c) => over(c.warning, 0.15, c.card), TEXT],
  ["text", "warningText / card(警告说明、主题诊断)", (c) => c.warningText, (c) => c.card, TEXT],
  ["text", "notice / notice 15% 叠在 card(新版本角标)", (c) => c.notice, (c) => over(c.notice, 0.15, c.card), TEXT],
  // 独立状态文字:直接放在卡片上(以前用的是填充色 success / warning,见文档 Q10)
  ["label", "successLabel / card(额度剩余 ≥50%、缓存命中率标题、保存成功提示)", (c) => c.successLabel, (c) => c.card, TEXT],
  ["label", "warningLabel / card(额度剩余 25–50%、定价「手动」、Agent 卡 ⚠)", (c) => c.warningLabel, (c) => c.card, TEXT],
  ["label", "dangerLabel / card(额度剩余 <25%)", (c) => c.dangerLabel, (c) => c.card, TEXT],
  // 统计卡六项指标:11px 标签,底色是 background 40% 叠在 card
  ...["input", "output", "cacheRead", "cacheWrite", "calls", "cost"].map((k) => [
    "stat",
    `统计标签 stat.${k} / MiniStat 底`,
    (c) => c[`stat.${k}`],
    (c) => over(c.background, 0.4, c.card),
    TEXT,
  ]),
  // 图形 / 控件(3:1)
  ["ui", "焦点环 ring / background", (c) => c.ring, (c) => c.background, UI],
  ...["success", "warning", "danger"].map((k) => [
    "ui",
    `进度条 ${k} / 轨道 muted 60% 叠在 card(缓存命中率、模型占比)`,
    (c) => c[k],
    (c) => over(c.muted, 0.6, c.card),
    UI,
  ]),
  ...["success", "warning", "danger"].map((k) => [
    "ui",
    `额度条 ${k} / 轨道 foreground 25% 叠在 card`,
    (c) => c[k],
    (c) => over(c.foreground, 0.25, c.card),
    UI,
  ]),
  // 设置页开关(所有开关同一个组件):滑块 switchThumb / switchThumbOff,
  // 轨道「开」= success,「关」= mutedForeground 30% 叠在 card(设置分区卡)
  ["switch", "开关「开」:滑块 switchThumb / 轨道 success", (c) => c.switchThumb, (c) => c.success, UI],
  ["switch", "开关「关」:滑块 switchThumbOff / 轨道 mutedForeground 30% 叠在 card", (c) => c.switchThumbOff, (c) => over(c.mutedForeground, 0.3, c.card), UI],
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

/** 检查一份清单的每个模式;返回是否全部达标 */
function checkManifest(manifest, source) {
  let ok = true;
  for (const mode of Object.keys(manifest.modes)) {
    const r = resolveTheme(manifest, mode, source);
    const c = colorsOf(r);
    let worstText = Infinity;
    let worstUi = Infinity;
    const switches = [];
    let fails = 0;
    console.log(`  · ${mode}`);
    const check = (group, label, fg, bg, min) => {
      if (!groups.includes(group)) return;
      const v = ratio(fg, bg);
      const pass = v >= min;
      if (!pass) fails++;
      if (min === TEXT) worstText = Math.min(worstText, v);
      else worstUi = Math.min(worstUi, v);
      if (group === "switch") switches.push(v.toFixed(2));
      console.log(`    ${pass ? "✓" : "✗"} ${v.toFixed(2).padStart(5)}:1 (≥${min}) ${label}`);
    };
    for (const [group, label, fg, bg, min] of PAIRS) check(group, label, fg(c), bg(c), min);
    // 图表:调色板与 Agent 品牌色画在卡片上(线、扇区、图标),按图形 3:1
    r.chart.palette.forEach((hex, i) =>
      check("chart", `图表 palette[${i}] ${hex} / card`, parseColor(hex), c.card, UI),
    );
    for (const [id, hex] of Object.entries(r.chart.agents)) {
      check("chart", `Agent ${id} ${hex} / card`, parseColor(hex), c.card, UI);
    }
    const parts = [];
    if (worstText !== Infinity) parts.push(`文字最低 ${worstText.toFixed(2)}:1`);
    if (worstUi !== Infinity) parts.push(`图形最低 ${worstUi.toFixed(2)}:1`);
    if (switches.length === 2) parts.push(`开关 开 ${switches[0]}:1 / 关 ${switches[1]}:1`);
    parts.push(fails === 0 ? "全部达标" : `${fails} 项低于阈值`);
    console.log(`    → ${parts.join(";")}`);
    if (fails > 0) ok = false;
  }
  return ok;
}

let failed = false;
if (only != null) console.log(`(只检查:${groups.join(", ")})`);
if (builtin) {
  console.log(`\n== 内置默认主题 ${OTR_THEME.id}(${OTR_THEME.name})`);
  if (!checkManifest(OTR_THEME, "builtin")) failed = true;
}
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
  const { manifest } = parseThemeFile(text, { reservedIds: reservedFor(file) });
  if (!manifest) {
    console.log("  ✗ 校验未通过,先用 scripts/theme-lint.mjs 修正");
    failed = true;
    continue;
  }
  if (!checkManifest(manifest, "user")) failed = true;
}
process.exit(failed ? 1 : 0);
