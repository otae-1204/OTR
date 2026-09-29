/**
 * 受限自定义 CSS(清单里的 `css` 字段)。
 *
 * 主题不能写任意 CSS,只能写一张**结构化**的表:
 *
 *   "css": {
 *     "card":        { "background-image": "linear-gradient(180deg, #fff 0%, #eee 100%)" },
 *     "card:hover":  { "box-shadow": "0 8px 20px rgba(0,0,0,.2)" },
 *     "card-title":  { "letter-spacing": "0.02em" }
 *   }
 *
 * - 键 = 钩子名(`THEME_PARTS`,对应组件上的 `data-theme-part`),可加一个状态后缀(`THEME_STATES`)。
 *   选择器由应用按钩子名生成(`[data-theme-part~="card"]:hover`),主题写不了选择器。
 * - 值 = 属性 → 值。属性只认白名单(`CSS_PROPERTIES`),每个属性有自己的值语法:
 *   值先经过分词器(`tokenize`,字符集极窄:字母数字、空格、`-+.%#(),/`),
 *   再按属性语法逐 token 匹配,最后由我们**重新拼出**字符串。分号、花括号、感叹号、
 *   引号、反斜杠、尖括号、`@`、`*`、`:` 根本进不了分词器,`url()` / `@import` /
 *   `expression()` / `image-set()` / `!important` / 注释无从构造。
 * - `var()` 只能引用本主题体系导出的变量(`--primary` 等,见 `varKind`)。
 * - `font-size` / `line-height` 只接受 0.8–1.2 的**倍率**(`1.1em` / `110%`;行高另可写 `1.1`),
 *   序列化成钩子元素上的 `--otr-font-scale` / `--otr-line-scale`,由 Tailwind 的字号 / 行高类相乘:
 *   每个元素都是「自己原来的字号 × 倍率」,嵌套钩子覆盖而不相乘(见 `SCALE_PROPS`)。
 * - 不合法的键 / 属性 / 值各自丢弃并报 warning(带 JSON 路径),不影响其余部分。
 *
 * 输出:`ThemeCss`(键 → 属性 → 归一化后的值);`serializeThemeCss` 把它变成
 * 一段样式文本,由 apply.ts 放进**唯一一个**受控 `<style id="otr-theme-css">` 里。
 */

import { parseColor, toHex, type Rgba } from "./color";
import { COLOR_TOKENS, STAT_TOKENS } from "./types";
import {
  formatShadowColor,
  normalizeFontFamily,
  normalizeShadow,
  normalizeTextShadow,
} from "./values";

// ---------------------------------------------------------------------------
// 钩子目录(稳定接口:名字一旦发布,同一 apiVersion 内不改名、不删除)
// ---------------------------------------------------------------------------

/**
 * 组件上的 `data-theme-part` 钩子。一个元素可以同时属于多个钩子(空格分隔,
 * 如 `"card stat-card"`),选择器用 `~=` 按词匹配。文档 §14.2 有每个钩子对应的界面位置。
 */
export const THEME_PARTS = [
  // 布局
  "app", // 应用根容器(页面底色与正文色;Cookie 教程窗口的根也是它)
  "header", // 顶栏
  "main", // 顶栏下方的内容区
  "filter-bar", // 仪表盘的筛选栏(Agent chip + 日期范围)
  "page-title", // 额度 / 设置页的大标题
  // 卡片(所有卡片都带 card,再各带一个更具体的钩子)
  "card",
  "card-title", // 卡片标题行
  "stat-card", // 仪表盘 Hero 统计卡
  "mini-stat", // 统计卡里的六个指标小格
  "agent-card", // Agent 卡片(可选中)
  "chart-card", // 趋势图 / 模型占比卡
  "table-card", // 会话明细卡
  "limit-card", // 额度页每一行账号卡
  "settings-section", // 设置页的分区卡
  // 图表
  "chart", // 图表绘图区容器
  "tooltip", // 图表浮层
  "legend", // 图例容器
  // 表格
  "table",
  "table-head",
  "table-row",
  // 控件
  "button", // 普通按钮(描边按钮)
  "button-primary", // 主按钮(实心主色)
  "segmented", // 分段控件的轨道(顶栏工具组、趋势图「对比 / 堆叠」、深浅模式)
  "segmented-button", // 分段控件里的按钮(选中态 = selected)
  "chip", // 筛选栏的 Agent / 范围 chip(选中态 = selected)
  "badge", // 小角标(「次请求」、套餐名、「待配置」、「可更新」等)
  "progress", // 进度条轨道
  "progress-fill", // 进度条填充
  "switch", // 开关(选中态 = selected)
  "switch-thumb", // 开关滑块
  "input", // 文本输入框 / 下拉框
  "empty-state", // 空状态提示框
] as const;
export type ThemePart = (typeof THEME_PARTS)[number];

/** 钩子名后可加的状态后缀:`card:hover` */
export const THEME_STATES = ["hover", "active", "focus", "disabled", "selected"] as const;
export type ThemeState = (typeof THEME_STATES)[number];

const PART_SET: ReadonlySet<string> = new Set(THEME_PARTS);
const STATE_SET: ReadonlySet<string> = new Set(THEME_STATES);

/** `css` 表最多多少个键(钩子[:状态]) */
export const MAX_CSS_KEYS = 64;
/** 每个键最多多少条声明 */
export const MAX_CSS_DECLS = 24;
/** 单个值的最大长度 */
export const MAX_CSS_VALUE_LEN = 256;

/** 键 → 属性 → 归一化后的值 */
export type ThemeCss = Record<string, Record<string, string>>;

// ---------------------------------------------------------------------------
// 分词器
// ---------------------------------------------------------------------------

export type CssToken =
  | { t: "ident"; v: string }
  | { t: "num"; v: number; unit: string }
  | { t: "hash"; v: string }
  | { t: "fn"; name: string; args: CssToken[] }
  | { t: "comma" }
  | { t: "slash" };

/** 值里允许出现的全部字符。没有引号、分号、花括号、感叹号、冒号、反斜杠、尖括号、@、*。 */
const SAFE_VALUE_CHARS = /^[A-Za-z0-9 \t\-+.%#(),/]*$/;
const NUM_RE = /^[+-]?(\d+(\.\d*)?|\.\d+)(%|[A-Za-z]+)?/;
const IDENT_RE = /^(?:--?)?[A-Za-z][A-Za-z0-9-]*/;

/** 把一个属性值切成 token;任何白名单外的字符或不成对的括号都返回 null */
export function tokenize(input: string): CssToken[] | null {
  if (!SAFE_VALUE_CHARS.test(input)) return null;
  const root: CssToken[] = [];
  const stack: CssToken[][] = [];
  let cur = root;
  let i = 0;
  const n = input.length;
  while (i < n) {
    const ch = input[i];
    if (ch === " " || ch === "\t") {
      i++;
      continue;
    }
    if (ch === ",") {
      cur.push({ t: "comma" });
      i++;
      continue;
    }
    if (ch === "/") {
      cur.push({ t: "slash" });
      i++;
      continue;
    }
    if (ch === ")") {
      const parent = stack.pop();
      if (!parent) return null;
      cur = parent;
      i++;
      continue;
    }
    if (ch === "(") return null; // 没有函数名的括号
    if (ch === "#") {
      let j = i + 1;
      while (j < n && /[0-9A-Fa-f]/.test(input[j])) j++;
      if (j === i + 1) return null;
      cur.push({ t: "hash", v: input.slice(i, j) });
      i = j;
      continue;
    }
    const rest = input.slice(i);
    const next = input[i + 1] ?? "";
    const numStart = /[0-9.]/.test(ch) || ((ch === "+" || ch === "-") && /[0-9.]/.test(next));
    if (numStart) {
      const m = NUM_RE.exec(rest);
      if (!m) return null;
      const unit = (m[3] ?? "").toLowerCase();
      const v = Number(m[0].slice(0, m[0].length - (m[3]?.length ?? 0)));
      if (!Number.isFinite(v) || m[0].length > 24) return null;
      cur.push({ t: "num", v, unit });
      i += m[0].length;
      continue;
    }
    const im = IDENT_RE.exec(rest);
    if (im) {
      const name = im[0].toLowerCase();
      i += im[0].length;
      if (input[i] === "(") {
        const fn: CssToken = { t: "fn", name, args: [] };
        cur.push(fn);
        stack.push(cur);
        cur = fn.args;
        i++;
      } else {
        cur.push({ t: "ident", v: name });
      }
      continue;
    }
    return null;
  }
  if (stack.length > 0) return null;
  return root;
}

// ---------------------------------------------------------------------------
// 基础值:数字 / 长度 / 颜色 / var()
// ---------------------------------------------------------------------------

/** camelCase → kebab-case:cardForeground → card-foreground(resolve.ts 也用它) */
export function kebab(s: string): string {
  return s.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);
}

const TRIPLET_VARS: ReadonlySet<string> = new Set([
  ...COLOR_TOKENS.map((k) => `--${kebab(k)}`),
  ...STAT_TOKENS.map((k) => `--stat-${kebab(k)}`),
]);

type VarKind = "triplet" | "hex" | "radius" | "shadow" | "font";

/**
 * 主题能引用哪些变量、它们是什么类型:
 * - 语义色 / 指标色是 HSL 三元组,只能写成 `hsl(var(--primary))` 或 `hsl(var(--primary) / 0.5)`;
 * - `--chart-N` / `--agent-<id>` 是 `#rrggbb`,可以直接当颜色;
 * - `--radius-*` / `--shadow*` / `--font-*` 只能整体作为对应属性的值。
 * 其它任何变量(应用内部的、页面上别的库的)都不允许。
 */
export function varKind(name: string): VarKind | null {
  if (TRIPLET_VARS.has(name)) return "triplet";
  if (/^--chart-([1-9]|1[0-6])$/.test(name)) return "hex";
  if (/^--agent-[a-z0-9][a-z0-9_-]{0,63}$/.test(name)) return "hex";
  if (/^--radius-(md|lg|xl)$/.test(name)) return "radius";
  if (name === "--shadow" || /^--shadow-(sm|md|lg)$/.test(name)) return "shadow";
  if (/^--font-(sans|mono)$/.test(name)) return "font";
  return null;
}

/** `var(--x)` 且 --x 是允许的变量 → 变量名;否则 null */
function varRef(tok: CssToken, kinds: readonly VarKind[]): string | null {
  if (tok.t !== "fn" || tok.name !== "var" || tok.args.length !== 1) return null;
  const a = tok.args[0];
  if (a.t !== "ident" || !a.v.startsWith("--")) return null;
  const k = varKind(a.v);
  return k && kinds.includes(k) ? a.v : null;
}

function fmtNum(v: number): string {
  const r = Math.round(v * 1000) / 1000;
  return Object.is(r, -0) ? "0" : String(r);
}

interface LengthOpts {
  /** 允许的单位(不含 `%`) */
  units: readonly string[];
  /** 各单位的绝对值上限 */
  max: Record<string, number>;
  /** 是否允许百分比(0–100) */
  pct?: boolean;
  /** 是否允许负数 */
  negative?: boolean;
}

/** 长度 token → 归一化字符串(`0`、`4px`、`0.5rem`、`50%`) */
function lengthTok(tok: CssToken, o: LengthOpts): string | null {
  if (tok.t !== "num") return null;
  if (tok.v < 0 && !o.negative) return null;
  if (tok.unit === "") return tok.v === 0 ? "0" : null;
  if (tok.unit === "%") {
    if (!o.pct || Math.abs(tok.v) > 100) return null;
    return `${fmtNum(tok.v)}%`;
  }
  if (!o.units.includes(tok.unit)) return null;
  const lim = o.max[tok.unit];
  if (lim === undefined || Math.abs(tok.v) > lim) return null;
  return tok.v === 0 ? "0" : `${fmtNum(tok.v)}${tok.unit}`;
}

/** 边框宽度:0–8px / 0–0.5rem */
const BORDER_WIDTH: LengthOpts = { units: ["px", "rem", "em"], max: { px: 8, rem: 0.5, em: 0.5 } };
/** 圆角:0–1000px / 0–64rem / 0–100% */
const RADIUS: LengthOpts = { units: ["px", "rem", "em"], max: { px: 1000, rem: 64, em: 64 }, pct: true };
/** 渐变色标位置:长度或百分比 */
const STOP_POS: LengthOpts = { units: ["px", "rem", "em"], max: { px: 1000, rem: 64, em: 64 }, pct: true };
/** 字距:±8px / ±0.5em */
const LETTER_SPACING: LengthOpts = {
  units: ["px", "rem", "em"],
  max: { px: 8, rem: 0.5, em: 0.5 },
  negative: true,
};
/** backdrop-filter: blur() 上限 40px */
const BLUR: LengthOpts = { units: ["px", "rem"], max: { px: 40, rem: 2.5 } };

function fmtColor(c: Rgba): string {
  return c.a >= 1 ? toHex(c) : formatShadowColor(c);
}

/** 只含数字 / 百分比 / 逗号 / 斜杠的函数参数 → 重新拼成字符串(交给 parseColor) */
function plainArgs(args: CssToken[]): string | null {
  const out: string[] = [];
  for (const a of args) {
    if (a.t === "num") {
      if (a.unit !== "" && a.unit !== "%" && a.unit !== "deg") return null;
      out.push(`${fmtNum(a.v)}${a.unit}`);
    } else if (a.t === "comma") {
      out.push(",");
    } else if (a.t === "slash") {
      out.push("/");
    } else {
      return null;
    }
  }
  return out.join(" ");
}

/** 透明度:0–1 的数或 0–100% */
function alphaTok(tok: CssToken): string | null {
  if (tok.t !== "num") return null;
  if (tok.unit === "" && tok.v >= 0 && tok.v <= 1) return fmtNum(tok.v);
  if (tok.unit === "%" && tok.v >= 0 && tok.v <= 100) return fmtNum(tok.v / 100);
  return null;
}

/**
 * 单个 token → 颜色字符串。接受:
 * `#hex`、`rgb()/rgba()/hsl()/hsla()`(纯数字参数)、`transparent`、`currentColor`、
 * `hsl(var(--语义色) [/ 透明度])`、`var(--chart-N)` / `var(--agent-id)`。
 */
export function colorTok(tok: CssToken): string | null {
  if (tok.t === "hash") {
    const c = parseColor(tok.v);
    return c ? fmtColor(c) : null;
  }
  if (tok.t === "ident") {
    if (tok.v === "transparent") return "transparent";
    if (tok.v === "currentcolor") return "currentColor";
    return null;
  }
  if (tok.t !== "fn") return null;
  if (tok.name === "var") {
    const v = varRef(tok, ["hex"]);
    return v ? `var(${v})` : null;
  }
  if (tok.name === "hsl" || tok.name === "hsla") {
    const first = tok.args[0];
    if (first && first.t === "fn" && first.name === "var") {
      const v = varRef(first, ["triplet"]);
      if (!v) return null;
      if (tok.args.length === 1) return `hsl(var(${v}))`;
      if (tok.args.length === 3 && tok.args[1].t === "slash") {
        const a = alphaTok(tok.args[2]);
        return a ? `hsl(var(${v}) / ${a})` : null;
      }
      return null;
    }
  }
  if (tok.name === "rgb" || tok.name === "rgba" || tok.name === "hsl" || tok.name === "hsla") {
    const inner = plainArgs(tok.args);
    if (inner == null) return null;
    const c = parseColor(`${tok.name}(${inner})`);
    return c ? fmtColor(c) : null;
  }
  return null;
}

// ---------------------------------------------------------------------------
// 属性语法
// ---------------------------------------------------------------------------

type Grammar = (toks: CssToken[]) => string | null;

function splitCommas(toks: CssToken[]): CssToken[][] {
  const groups: CssToken[][] = [[]];
  for (const t of toks) {
    if (t.t === "comma") groups.push([]);
    else groups[groups.length - 1].push(t);
  }
  return groups;
}

function single(f: (tok: CssToken) => string | null): Grammar {
  return (toks) => (toks.length === 1 ? f(toks[0]) : null);
}

function keyword(...allowed: string[]): Grammar {
  return single((tok) => (tok.t === "ident" && allowed.includes(tok.v) ? tok.v : null));
}

/** 1–max 个同类值(边框宽度、圆角等的四边写法) */
function repeat(f: (tok: CssToken) => string | null, max: number): Grammar {
  return (toks) => {
    if (toks.length < 1 || toks.length > max) return null;
    const out: string[] = [];
    for (const t of toks) {
      const v = f(t);
      if (v == null) return null;
      out.push(v);
    }
    return out.join(" ");
  };
}

const BORDER_STYLES = ["none", "solid", "dashed", "dotted", "double"];

function borderStyleTok(tok: CssToken): string | null {
  return tok.t === "ident" && BORDER_STYLES.includes(tok.v) ? tok.v : null;
}

/** `border` / `border-<边>` 简写:[宽度] [样式] [颜色] 任意顺序,或 `none` */
const borderShorthand: Grammar = (toks) => {
  if (toks.length === 1 && toks[0].t === "ident" && toks[0].v === "none") return "none";
  if (toks.length < 1 || toks.length > 3) return null;
  let width: string | null = null;
  let style: string | null = null;
  let color: string | null = null;
  for (const t of toks) {
    const w = lengthTok(t, BORDER_WIDTH);
    if (w != null && width == null) {
      width = w;
      continue;
    }
    const s = borderStyleTok(t);
    if (s != null && style == null) {
      style = s;
      continue;
    }
    const c = colorTok(t);
    if (c != null && color == null) {
      color = c;
      continue;
    }
    return null;
  }
  return [width, style, color].filter((x) => x != null).join(" ");
};

const radiusTok = (tok: CssToken) => lengthTok(tok, RADIUS);

const borderRadius: Grammar = (toks) => {
  if (toks.length === 1) {
    const v = varRef(toks[0], ["radius"]);
    if (v) return `var(${v})`;
  }
  return repeat(radiusTok, 4)(toks);
};

const ANGLE_UNITS: Record<string, number> = { deg: 360, turn: 1, rad: 6.2832, grad: 400 };
const SIDES = ["top", "bottom", "left", "right"];

/** 渐变色标:颜色 [位置]{0,2} */
function colorStop(toks: CssToken[]): string | null {
  if (toks.length < 1 || toks.length > 3) return null;
  const c = colorTok(toks[0]);
  if (c == null) return null;
  const out = [c];
  for (const t of toks.slice(1)) {
    const p = lengthTok(t, STOP_POS);
    if (p == null) return null;
    out.push(p);
  }
  return out.join(" ");
}

/** linear-gradient 的方向:角度,或 `to <边> [<边>]` */
function linearDirection(toks: CssToken[]): string | null {
  if (toks.length === 1 && toks[0].t === "num") {
    const t = toks[0];
    const lim = ANGLE_UNITS[t.unit];
    if (lim === undefined || Math.abs(t.v) > lim) return null;
    return `${fmtNum(t.v)}${t.unit}`;
  }
  if (toks.length >= 2 && toks.length <= 3 && toks[0].t === "ident" && toks[0].v === "to") {
    const sides: string[] = [];
    for (const t of toks.slice(1)) {
      if (t.t !== "ident" || !SIDES.includes(t.v) || sides.includes(t.v)) return null;
      sides.push(t.v);
    }
    return `to ${sides.join(" ")}`;
  }
  return null;
}

const RADIAL_WORDS = [
  "circle",
  "ellipse",
  "closest-side",
  "farthest-side",
  "closest-corner",
  "farthest-corner",
  "at",
  "center",
  ...SIDES,
];

/** radial-gradient 的形状 / 大小 / 位置:只允许固定关键字与长度,不含颜色 */
function radialPrelude(toks: CssToken[]): string | null {
  if (toks.length < 1 || toks.length > 6) return null;
  const out: string[] = [];
  for (const t of toks) {
    if (t.t === "ident" && RADIAL_WORDS.includes(t.v)) {
      out.push(t.v);
      continue;
    }
    const l = lengthTok(t, STOP_POS);
    if (l == null) return null;
    out.push(l);
  }
  return out.join(" ");
}

const MAX_GRADIENTS = 4;
const MAX_STOPS = 16;

function gradient(tok: CssToken): string | null {
  if (tok.t !== "fn") return null;
  if (tok.name !== "linear-gradient" && tok.name !== "radial-gradient") return null;
  const groups = splitCommas(tok.args);
  if (groups.length < 2) return null;
  const parts: string[] = [];
  let i = 0;
  const prelude = tok.name === "linear-gradient" ? linearDirection(groups[0]) : radialPrelude(groups[0]);
  if (prelude != null) {
    parts.push(prelude);
    i = 1;
  }
  let stops = 0;
  for (; i < groups.length; i++) {
    const s = colorStop(groups[i]);
    if (s == null) return null;
    parts.push(s);
    stops++;
  }
  if (stops < 2 || stops > MAX_STOPS) return null;
  return `${tok.name}(${parts.join(", ")})`;
}

const backgroundImage: Grammar = (toks) => {
  if (toks.length === 1 && toks[0].t === "ident" && toks[0].v === "none") return "none";
  const groups = splitCommas(toks);
  if (groups.length > MAX_GRADIENTS) return null;
  const out: string[] = [];
  for (const g of groups) {
    if (g.length !== 1) return null;
    const v = gradient(g[0]);
    if (v == null) return null;
    out.push(v);
  }
  return out.join(", ");
};

const opacity: Grammar = single(alphaTok);

/** 字号 / 行高倍率的上下限:组件原值的 ±20% */
export const MIN_TEXT_SCALE = 0.8;
export const MAX_TEXT_SCALE = 1.2;

/**
 * 倍率:`em`(字号)或无单位数(行高)0.8–1.2,或百分比 80%–120%。其它一律拒绝:
 * px / rem / vw、calc() / var() / clamp()、`larger` / `inherit` 之类关键字、多个值。
 * 归一化后保留原单位(`1.1em`、`110%`、`1.1`),序列化时再换成纯倍率。
 */
function scaleGrammar(unit: "em" | ""): Grammar {
  return single((tok) => {
    if (tok.t !== "num") return null;
    if (tok.unit === "%") {
      if (tok.v < MIN_TEXT_SCALE * 100 || tok.v > MAX_TEXT_SCALE * 100) return null;
      return `${fmtNum(tok.v)}%`;
    }
    if (tok.unit !== unit || tok.v < MIN_TEXT_SCALE || tok.v > MAX_TEXT_SCALE) return null;
    return `${fmtNum(tok.v)}${unit}`;
  });
}

/** 归一化后的倍率值(`1.1em` / `110%` / `1.1`)→ 纯数字字符串 */
function scaleFactor(value: string): string | null {
  const m = /^(\d+(?:\.\d+)?)(em|%)?$/.exec(value);
  if (!m) return null;
  const v = Number(m[1]) / (m[2] === "%" ? 100 : 1);
  return v >= MIN_TEXT_SCALE && v <= MAX_TEXT_SCALE ? fmtNum(v) : null;
}

/**
 * 这两个属性不按字面写出:`font-size: 1.1em` 会相对**父元素**计算,钩子元素自己的字号类
 * (`text-xs`)被覆盖后可能一下变成父元素的 1.1 倍(12px → 17.6px),±20% 根本守不住。
 * 所以只在钩子元素上设置倍率变量,由 tailwind.config.js 里每个字号 / 行高类乘上它。
 */
const SCALE_PROPS: Record<string, string> = {
  "font-size": "--otr-font-scale",
  "line-height": "--otr-line-scale",
};

const fontWeight: Grammar = single((tok) => {
  if (tok.t === "ident") return ["normal", "bold", "lighter", "bolder"].includes(tok.v) ? tok.v : null;
  if (tok.t === "num" && tok.unit === "" && Number.isInteger(tok.v) && tok.v >= 1 && tok.v <= 1000) {
    return String(tok.v);
  }
  return null;
});

const letterSpacing: Grammar = single((tok) => {
  if (tok.t === "ident" && tok.v === "normal") return "normal";
  return lengthTok(tok, LETTER_SPACING);
});

const DECORATION_LINES = ["underline", "overline", "line-through"];
const textDecorationLine: Grammar = (toks) => {
  if (toks.length === 1 && toks[0].t === "ident" && toks[0].v === "none") return "none";
  if (toks.length < 1 || toks.length > 2) return null;
  const out: string[] = [];
  for (const t of toks) {
    if (t.t !== "ident" || !DECORATION_LINES.includes(t.v) || out.includes(t.v)) return null;
    out.push(t.v);
  }
  return out.join(" ");
};

/** 0–3 的数或 0–300% */
function ratioArg(tok: CssToken): string | null {
  if (tok.t !== "num") return null;
  if (tok.unit === "" && tok.v >= 0 && tok.v <= 3) return fmtNum(tok.v);
  if (tok.unit === "%" && tok.v >= 0 && tok.v <= 300) return `${fmtNum(tok.v)}%`;
  return null;
}

const backdropFilter: Grammar = (toks) => {
  if (toks.length === 1 && toks[0].t === "ident" && toks[0].v === "none") return "none";
  if (toks.length < 1 || toks.length > 3) return null;
  const out: string[] = [];
  for (const t of toks) {
    if (t.t !== "fn" || t.args.length !== 1) return null;
    if (t.name === "blur") {
      const l = lengthTok(t.args[0], BLUR);
      if (l == null) return null;
      out.push(`blur(${l})`);
    } else if (t.name === "saturate" || t.name === "brightness" || t.name === "contrast") {
      const r = ratioArg(t.args[0]);
      if (r == null) return null;
      out.push(`${t.name}(${r})`);
    } else {
      return null;
    }
  }
  return out.join(" ");
};

/** 分词后按属性语法校验的属性(font-family / 阴影走字符串路径,见 normalizeDeclaration) */
const GRAMMARS: Record<string, Grammar> = {
  color: single(colorTok),
  "background-color": single(colorTok),
  "background-image": backgroundImage,
  "background-clip": keyword("border-box", "padding-box", "content-box", "text"),
  border: borderShorthand,
  "border-top": borderShorthand,
  "border-right": borderShorthand,
  "border-bottom": borderShorthand,
  "border-left": borderShorthand,
  "border-color": repeat(colorTok, 4),
  "border-top-color": single(colorTok),
  "border-right-color": single(colorTok),
  "border-bottom-color": single(colorTok),
  "border-left-color": single(colorTok),
  "border-width": repeat((t) => lengthTok(t, BORDER_WIDTH), 4),
  "border-top-width": single((t) => lengthTok(t, BORDER_WIDTH)),
  "border-right-width": single((t) => lengthTok(t, BORDER_WIDTH)),
  "border-bottom-width": single((t) => lengthTok(t, BORDER_WIDTH)),
  "border-left-width": single((t) => lengthTok(t, BORDER_WIDTH)),
  "border-style": repeat(borderStyleTok, 4),
  "border-top-style": single(borderStyleTok),
  "border-right-style": single(borderStyleTok),
  "border-bottom-style": single(borderStyleTok),
  "border-left-style": single(borderStyleTok),
  "border-radius": borderRadius,
  "border-top-left-radius": repeat(radiusTok, 2),
  "border-top-right-radius": repeat(radiusTok, 2),
  "border-bottom-right-radius": repeat(radiusTok, 2),
  "border-bottom-left-radius": repeat(radiusTok, 2),
  "outline-color": single(colorTok),
  opacity,
  "font-weight": fontWeight,
  "font-style": keyword("normal", "italic", "oblique"),
  "letter-spacing": letterSpacing,
  "text-transform": keyword("none", "uppercase", "lowercase", "capitalize"),
  "text-decoration-line": textDecorationLine,
  "text-decoration-style": keyword("solid", "double", "dotted", "dashed", "wavy"),
  "text-decoration-color": single(colorTok),
  "backdrop-filter": backdropFilter,
  "font-size": scaleGrammar("em"),
  "line-height": scaleGrammar(""),
};

/** 走字符串语法的属性 */
const STRING_PROPS = ["font-family", "box-shadow", "text-shadow"] as const;

/** 允许的全部属性(白名单) */
export const CSS_PROPERTIES: readonly string[] = [...Object.keys(GRAMMARS), ...STRING_PROPS];
const PROPERTY_SET: ReadonlySet<string> = new Set(CSS_PROPERTIES);

/** 常见但被拒绝的属性 → 给作者的提示 */
const PROPERTY_HINTS: Record<string, string> = {
  background: "请改用 background-color / background-image",
  transition: "过渡由界面自己控制,不开放",
  animation: "动画不开放",
  transform: "变换会破坏布局,不开放",
  filter: "只开放 backdrop-filter",
  outline: "只开放 outline-color",
};

/** 语法较窄、容易写错的属性 → 拒绝时给作者的提示 */
const VALUE_HINTS: Record<string, string> = {
  "font-size": "只接受相对组件原字号的倍率 0.8em–1.2em 或 80%–120%",
  "line-height": "只接受相对组件原行高的倍率 0.8–1.2 或 80%–120%",
};

/**
 * 校验并归一化一条声明。属性不在白名单 → `{ ok: false, reason }`;值不合法同理。
 * 成功时返回重新格式化后的值。
 */
export function normalizeDeclaration(
  prop: string,
  raw: unknown,
): { ok: true; value: string } | { ok: false; reason: string } {
  if (!PROPERTY_SET.has(prop)) {
    const hint = PROPERTY_HINTS[prop];
    return { ok: false, reason: hint ? `不支持的属性(${hint}),已忽略` : "不支持的属性,已忽略" };
  }
  if (typeof raw !== "string") return { ok: false, reason: "值应为字符串,已忽略" };
  const s = raw.trim();
  if (!s) return { ok: false, reason: "值为空,已忽略" };
  if (s.length > MAX_CSS_VALUE_LEN) {
    return { ok: false, reason: `值超过 ${MAX_CSS_VALUE_LEN} 个字符,已忽略` };
  }
  // 引号只允许出现在 font-family 里,所以它不走分词器,走自己的白名单
  if (prop === "font-family") {
    const m = /^var\((--font-(?:sans|mono))\)$/.exec(s);
    if (m) return { ok: true, value: `var(${m[1]})` };
    const f = normalizeFontFamily(s);
    return f ? { ok: true, value: f } : { ok: false, reason: "字体族不合法(见 font token 的规则),已忽略" };
  }
  const toks = tokenize(s);
  if (!toks || toks.length === 0) return { ok: false, reason: "值含有不允许的字符或括号不成对,已忽略" };
  if (prop === "box-shadow" || prop === "text-shadow") {
    if (prop === "box-shadow" && toks.length === 1) {
      const v = varRef(toks[0], ["shadow"]);
      if (v) return { ok: true, value: `var(${v})` };
    }
    const sh = prop === "box-shadow" ? normalizeShadow(s) : normalizeTextShadow(s);
    return sh ? { ok: true, value: sh } : { ok: false, reason: "阴影语法不合法,已忽略" };
  }
  const grammar = GRAMMARS[prop];
  const value = grammar ? grammar(toks) : null;
  if (value != null) return { ok: true, value };
  const hint = VALUE_HINTS[prop];
  return { ok: false, reason: hint ? `值不符合该属性的语法(${hint}),已忽略` : "值不符合该属性的语法,已忽略" };
}

// ---------------------------------------------------------------------------
// 键(钩子[:状态])→ 选择器
// ---------------------------------------------------------------------------

const KEY_RE = /^([a-z][a-z0-9-]*)(?::([a-z-]+))?$/;

export function parseCssKey(key: string): { part: ThemePart; state: ThemeState | null } | null {
  const m = KEY_RE.exec(key);
  if (!m) return null;
  if (!PART_SET.has(m[1])) return null;
  if (m[2] !== undefined && !STATE_SET.has(m[2])) return null;
  return { part: m[1] as ThemePart, state: (m[2] as ThemeState | undefined) ?? null };
}

/** 应用生成的选择器;键不合法返回 null */
export function selectorForKey(key: string): string | null {
  const k = parseCssKey(key);
  if (!k) return null;
  const base = `[data-theme-part~="${k.part}"]`;
  switch (k.state) {
    case null:
      return base;
    case "hover":
      return `${base}:hover`;
    case "active":
      return `${base}:active`;
    case "focus":
      return `${base}:focus-visible`;
    case "disabled":
      return `${base}:disabled`;
    case "selected":
      return `${base}[data-theme-state~="selected"]`;
    default:
      return null;
  }
}

// ---------------------------------------------------------------------------
// 校验 / 合并 / 序列化
// ---------------------------------------------------------------------------

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/**
 * 校验清单里的 `css` 字段。不合法的键 / 属性 / 值各自丢弃并通过 `warn` 报告(带 JSON 路径);
 * 返回归一化后的表(可能为空对象),`raw` 未提供时返回 undefined。
 */
export function validateCss(
  raw: unknown,
  path: string,
  warn: (path: string, message: string) => void,
): ThemeCss | undefined {
  if (raw === undefined) return undefined;
  if (!isPlainObject(raw)) {
    warn(path, "应为 { 钩子[:状态]: { 属性: 值 } } 对象,已忽略");
    return undefined;
  }
  const out: ThemeCss = {};
  const keys = Object.keys(raw);
  if (keys.length > MAX_CSS_KEYS) {
    warn(path, `最多 ${MAX_CSS_KEYS} 个钩子条目,多出的已忽略`);
  }
  for (const key of keys.slice(0, MAX_CSS_KEYS)) {
    const keyPath = `${path}.${key}`;
    const parsed = parseCssKey(key);
    if (!parsed) {
      const m = KEY_RE.exec(key);
      if (m && PART_SET.has(m[1])) {
        warn(keyPath, `未知状态「${m[2]}」(可用:${THEME_STATES.join(" / ")}),已忽略`);
      } else {
        warn(keyPath, "未知钩子,已忽略(钩子目录见 docs/theme_interface.md §14.2)");
      }
      continue;
    }
    const block = raw[key];
    if (!isPlainObject(block)) {
      warn(keyPath, "应为 { 属性: 值 } 对象,已忽略");
      continue;
    }
    const decls: Record<string, string> = {};
    const props = Object.keys(block);
    if (props.length > MAX_CSS_DECLS) {
      warn(keyPath, `每个钩子最多 ${MAX_CSS_DECLS} 条声明,多出的已忽略`);
    }
    for (const rawProp of props.slice(0, MAX_CSS_DECLS)) {
      const prop = rawProp.trim().toLowerCase();
      const r = normalizeDeclaration(prop, block[rawProp]);
      if (!r.ok) {
        warn(`${keyPath}.${rawProp}`, r.reason);
        continue;
      }
      decls[prop] = r.value;
    }
    if (Object.keys(decls).length === 0) continue;
    out[key] = decls;
  }
  return out;
}

/** 按「键 → 属性」粒度叠加:后者覆盖前者 */
export function mergeCss(base: ThemeCss | undefined, over: ThemeCss | undefined): ThemeCss | undefined {
  if (!over) return base;
  if (!base) return over;
  const out: ThemeCss = {};
  for (const k of Object.keys(base)) out[k] = { ...base[k] };
  for (const k of Object.keys(over)) out[k] = { ...(out[k] ?? {}), ...over[k] };
  return out;
}

/** 序列化时对值再做一道字符集检查(值本应已由 normalizeDeclaration 生成) */
const SAFE_OUTPUT_RE = /^[\p{L}\p{N} _"'.,\-+%#()/]*$/u;

/** 需要同时写带前缀版本的属性(WebView2 / WebKit) */
const VENDOR_PREFIXED: Record<string, string> = {
  "backdrop-filter": "-webkit-backdrop-filter",
  "background-clip": "-webkit-background-clip",
};

/**
 * 把归一化后的表变成样式文本。选择器由钩子名生成、属性只出白名单(`font-size` / `line-height`
 * 写成倍率变量,见 `SCALE_PROPS`),每个值在这里**再跑一遍**
 * `normalizeDeclaration`(归一化是幂等的,对合法表零开销;对被篡改的表则把非法值丢掉),
 * 最后再过一次字符集检查。因此输出里不可能出现 `;` `{` `}` `<` `!` 之类的结构字符。
 */
export function serializeThemeCss(css: ThemeCss | undefined): string {
  if (!css) return "";
  const rules: string[] = [];
  for (const key of Object.keys(css)) {
    const selector = selectorForKey(key);
    if (!selector) continue;
    const decls: string[] = [];
    const block = css[key];
    if (typeof block !== "object" || block === null) continue;
    for (const [prop, raw] of Object.entries(block)) {
      if (!PROPERTY_SET.has(prop)) continue;
      const r = normalizeDeclaration(prop, raw);
      if (!r.ok) continue;
      const value = r.value;
      if (!SAFE_OUTPUT_RE.test(value) || value.length > MAX_CSS_VALUE_LEN) continue;
      const scaleVar = SCALE_PROPS[prop];
      if (scaleVar) {
        const factor = scaleFactor(value);
        if (factor != null) decls.push(`${scaleVar}: ${factor}`);
        continue;
      }
      const prefixed = VENDOR_PREFIXED[prop];
      if (prefixed) decls.push(`${prefixed}: ${value}`);
      decls.push(`${prop}: ${value}`);
    }
    if (decls.length === 0) continue;
    rules.push(`${selector} {\n  ${decls.join(";\n  ")};\n}`);
  }
  return rules.join("\n");
}

/** 从不可信来源(如 localStorage 缓存)恢复一张表:整张重新校验,丢掉一切不合法的 */
export function sanitizeCss(raw: unknown): ThemeCss | undefined {
  return validateCss(raw, "", () => undefined);
}
