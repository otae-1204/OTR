#!/usr/bin/env node
/**
 * 主题接口的自检用例(以受限自定义 CSS 校验器为主)。
 *
 *   node scripts/theme-css-selftest.mjs        (或 npm run test:theme)
 *
 * 仓库没有前端测试框架,这里用最朴素的断言跑一遍 src/theme/ 的关键行为:
 * - css.ts:分词器的字符集、属性白名单、值语法、var() 越权、钩子/状态键、限额、序列化输出的形状,
 *   以及各种注入尝试(分号 / 花括号 / url() / @import / expression() / !important / 注释 / 引号 …)
 *   全部被丢弃;font-size / line-height 只接受 ±20% 的倍率并写成倍率变量;
 * - 后加 token 的回退链(successLabel / switchThumb …)、单模式主题不改偏好模式(decide);
 * - 与代码之外的几处保持同步:docs/theme.schema.json、src/index.css 的兜底变量、
 *   tailwind.config.js 的字号 / 行高倍率、组件里没有绕开倍率的任意字号、Rust 侧的复位脚本常量。
 * 跑的是应用里同一份代码(通过 esbuild 打包导入)。退出码:有失败为 1。
 */
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { loadThemeModule, repoRoot } from "./lib/load-theme.mjs";

const T = await loadThemeModule("theme-selftest");
const {
  THEME_PARTS,
  THEME_STATES,
  CSS_PROPERTIES,
  tokenize,
  normalizeDeclaration,
  validateCss,
  mergeCss,
  serializeThemeCss,
  selectorForKey,
  sanitizeCss,
  validateManifest,
  parseThemeFile,
  resolveTheme,
  decide,
  parseColor,
  COLOR_TOKENS,
  COLOR_FALLBACKS,
  OTR_THEME,
  THEME_CACHE_KEY,
  THEME_STYLE_ID,
  THEME_RESET_EVENT,
} = T;

let pass = 0;
let fail = 0;
const failures = [];

function check(name, ok, detail) {
  if (ok) {
    pass++;
  } else {
    fail++;
    failures.push(`${name}${detail !== undefined ? ` — ${detail}` : ""}`);
  }
}

/** 断言某条声明被接受,且归一化结果等于 expected(省略 expected 时只看接受) */
function accepts(prop, value, expected) {
  const r = normalizeDeclaration(prop, value);
  if (!r.ok) return check(`accepts ${prop}: ${value}`, false, r.reason);
  // 归一化必须幂等:序列化时会把值再归一化一遍
  const again = normalizeDeclaration(prop, r.value);
  check(`idempotent ${prop}: ${r.value}`, again.ok && again.value === r.value, again.ok ? `→ ${JSON.stringify(again.value)}` : again.reason);
  if (expected !== undefined) {
    check(`accepts ${prop}: ${value} → ${expected}`, r.value === expected, `got ${JSON.stringify(r.value)}`);
  } else {
    check(`accepts ${prop}: ${value}`, true);
  }
}

/** 断言某条声明被拒绝 */
function rejects(prop, value) {
  const r = normalizeDeclaration(prop, value);
  check(`rejects ${prop}: ${JSON.stringify(value)}`, !r.ok, `accepted as ${JSON.stringify(r.ok ? r.value : null)}`);
}

// ---------------------------------------------------------------------------
// 1. 分词器:字符集与括号
// ---------------------------------------------------------------------------
for (const bad of [
  "red;",
  "#fff; position: fixed",
  "#fff }",
  "#fff } body { display: none",
  "#fff !important",
  "#fff /* c */",
  '"#fff"',
  "'#fff'",
  "#fff\\",
  "#fff<script>",
  "#fff@import",
  "#fff*",
  "#fff:hover",
  "#fff\u0000",
  "#fff\n",
  "＃fff", // 全角
  "rgb(0,0,0",
  "rgb 0,0,0)",
  "(0)",
  "#",
  "--",
]) {
  check(`tokenize rejects ${JSON.stringify(bad)}`, tokenize(bad) === null);
}
check("tokenize accepts nested fn", Array.isArray(tokenize("linear-gradient(to right, rgb(0 0 0 / 50%), #fff 10%)")));
check("tokenize lowercases ident", tokenize("SOLID")?.[0]?.v === "solid");
check("tokenize number unit", (() => {
  const t = tokenize("-1.5PX");
  return t?.[0]?.t === "num" && t[0].v === -1.5 && t[0].unit === "px";
})());

// ---------------------------------------------------------------------------
// 2. 属性白名单
// ---------------------------------------------------------------------------
for (const p of [
  "position", "display", "width", "height", "min-width", "max-height", "margin", "padding",
  "z-index", "transform", "content", "animation", "transition", "background", "font-size",
  "line-height", "filter", "outline", "pointer-events", "visibility", "overflow", "inset",
  "top", "left", "float", "clip-path", "mask", "mask-image", "border-image", "list-style",
  "cursor", "user-select", "-webkit-text-fill-color", "-webkit-appearance", "appearance",
  "background-size", "background-position", "background-attachment", "grid-template-columns",
  "flex", "order", "gap", "will-change", "contain", "isolation",
]) {
  rejects(p, "1px");
}
check("CSS_PROPERTIES has no layout props", !CSS_PROPERTIES.some((p) => /^(position|display|width|height|margin|padding|z-index|transform|content|animation)$/.test(p)));
check("CSS_PROPERTIES is non-empty whitelist", CSS_PROPERTIES.length >= 30 && CSS_PROPERTIES.includes("background-image"));

// 属性名大小写 / 前后空白由 validateCss 归一化,normalizeDeclaration 本身只认小写
rejects("Color", "#fff");
rejects("__proto__", "#fff");
rejects("constructor", "#fff");

// ---------------------------------------------------------------------------
// 3. 颜色
// ---------------------------------------------------------------------------
accepts("color", "#FFF", "#ffffff");
accepts("color", "#abcdef", "#abcdef");
accepts("color", "#abcdef80", "rgb(171 205 239 / 0.502)");
accepts("color", "rgb(255, 0, 0)", "#ff0000");
accepts("color", "rgba(0, 0, 0, .5)", "rgb(0 0 0 / 0.5)");
accepts("color", "rgb(0 0 0 / 50%)", "rgb(0 0 0 / 0.5)");
accepts("color", "hsl(210 100% 56%)");
accepts("color", "hsl(210deg, 100%, 56%)");
accepts("color", "transparent", "transparent");
accepts("color", "currentColor", "currentColor");
accepts("color", "CURRENTCOLOR", "currentColor");
rejects("color", "red");
rejects("color", "inherit");
rejects("color", "#fff #000");
rejects("color", "rgb(0, 0, 0, 0, 0)");
rejects("color", "rgb(var(--primary))");
rejects("color", "color-mix(in srgb, #fff, #000)");
rejects("color", "lab(50% 0 0)");
rejects("color", "url(#fff)");
rejects("color", "attr(data-x)");
rejects("color", "env(safe-area-inset-top)");
rejects("color", "1e999");
rejects("color", "");
rejects("color", "   ");
rejects("color", 42);
rejects("color", null);
rejects("color", ["#fff"]);

// ---------------------------------------------------------------------------
// 4. var() 只能引用本主题体系的变量
// ---------------------------------------------------------------------------
accepts("color", "hsl(var(--primary))", "hsl(var(--primary))");
accepts("color", "hsl(var(--primary) / 0.4)", "hsl(var(--primary) / 0.4)");
accepts("color", "hsl(var(--primary) / 40%)", "hsl(var(--primary) / 0.4)");
accepts("color", "hsl(var(--stat-cache-read))", "hsl(var(--stat-cache-read))");
accepts("color", "hsl(var(--success-text))");
accepts("background-color", "var(--chart-1)", "var(--chart-1)");
accepts("background-color", "var(--chart-16)");
accepts("color", "var(--agent-dsh)", "var(--agent-dsh)");
accepts("color", "var(--agent-custom-codebuddy)");
accepts("border-radius", "var(--radius-lg)", "var(--radius-lg)");
accepts("box-shadow", "var(--shadow-md)", "var(--shadow-md)");
accepts("box-shadow", "var(--shadow)", "var(--shadow)");
accepts("font-family", "var(--font-mono)", "var(--font-mono)");
rejects("color", "var(--primary)"); // 三元组不能直接当颜色
rejects("color", "hsl(var(--chart-1))"); // hex 不能塞进 hsl
rejects("color", "var(--chart-17)");
rejects("color", "var(--chart-0)");
rejects("color", "var(--foo)");
rejects("color", "var(--tw-ring-color)");
rejects("color", "var(--radius-lg)");
rejects("color", "var(--primary, red)");
rejects("color", "hsl(var(--primary), 50%)");
rejects("color", "hsl(var(--primary) / var(--x))");
rejects("border-radius", "var(--primary)");
rejects("box-shadow", "var(--radius-lg)");
rejects("box-shadow", "0 0 0 1px var(--chart-1)");
rejects("font-family", "var(--font-serif)");
rejects("font-family", "var(--font-mono), monospace");
rejects("opacity", "var(--x)");
rejects("color", "var(--agent-a.b)"); // 变量名里不能有点
rejects("color", "var()");
rejects("color", "var(primary)");

// ---------------------------------------------------------------------------
// 5. background-image:只允许 linear/radial-gradient
// ---------------------------------------------------------------------------
accepts(
  "background-image",
  "linear-gradient(180deg, rgba(255,255,255,0.5) 0%, rgba(255,255,255,0) 60%)",
  "linear-gradient(180deg, rgb(255 255 255 / 0.5) 0%, rgb(255 255 255 / 0) 60%)",
);
accepts("background-image", "linear-gradient(to right, #fff, #000)", "linear-gradient(to right, #ffffff, #000000)");
accepts("background-image", "linear-gradient(to bottom left, #fff 0, #000 100%)");
accepts("background-image", "linear-gradient(#fff, #000)", "linear-gradient(#ffffff, #000000)");
accepts("background-image", "linear-gradient(0.25turn, #fff, #000)");
accepts("background-image", "linear-gradient(135deg, hsl(var(--primary) / 0.2), transparent)");
accepts("background-image", "radial-gradient(circle at top left, #fff, #000)", "radial-gradient(circle at top left, #ffffff, #000000)");
accepts("background-image", "radial-gradient(#fff, #000)");
accepts("background-image", "radial-gradient(ellipse farthest-corner at 20% 30%, #fff 0%, #000 100%)");
accepts("background-image", "radial-gradient(120px 80px at center, #fff, #000)");
accepts("background-image", "linear-gradient(#fff, #000), radial-gradient(#111, #222)");
accepts("background-image", "none", "none");
rejects("background-image", "url(https://evil.example/x.png)");
rejects("background-image", "url('x.png')");
rejects("background-image", "image-set(url(x.png) 1x)");
rejects("background-image", "-webkit-image-set(url(x.png) 1x)");
rejects("background-image", "conic-gradient(#fff, #000)");
rejects("background-image", "repeating-linear-gradient(#fff, #000)");
rejects("background-image", "linear-gradient(#fff)"); // 至少两个色标
rejects("background-image", "linear-gradient(to nowhere, #fff, #000)");
rejects("background-image", "linear-gradient(to top top, #fff, #000)");
rejects("background-image", "linear-gradient(999deg, #fff, #000)");
rejects("background-image", "linear-gradient(red, blue)");
rejects("background-image", "linear-gradient(#fff, url(x))");
rejects("background-image", "linear-gradient(#fff, var(--foo))");
rejects("background-image", "linear-gradient(#fff 0% 50% 100%, #000)");
rejects("background-image", "linear-gradient(#fff, #000) url(x)");
rejects("background-image", "element(#x)");
rejects("background-image", "paint(foo)");
rejects("background-image", "cross-fade(#fff, #000)");
rejects("background-image", "linear-gradient(#fff, #000), linear-gradient(#fff, #000), linear-gradient(#fff, #000), linear-gradient(#fff, #000), linear-gradient(#fff, #000)");
{
  const many = `linear-gradient(${Array.from({ length: 17 }, () => "#fff").join(", ")})`;
  rejects("background-image", many);
}

// ---------------------------------------------------------------------------
// 6. 边框 / 圆角 / 阴影 / 其它
// ---------------------------------------------------------------------------
accepts("border", "1px solid #fff", "1px solid #ffffff");
accepts("border", "solid #fff 2px", "2px solid #ffffff");
accepts("border", "none", "none");
accepts("border", "dashed", "dashed");
accepts("border-top", "1px solid hsl(var(--border))");
rejects("border", "1px solid red");
rejects("border", "20px solid #fff");
rejects("border", "1px 2px solid #fff");
rejects("border", "1px solid #fff #000");
rejects("border", "1px groove #fff");
rejects("border", "1px solid #fff; position: fixed");
accepts("border-color", "#fff #000", "#ffffff #000000");
accepts("border-width", "1px 0 2px 0", "1px 0 2px 0");
rejects("border-width", "1px 0 2px 0 1px");
rejects("border-width", "-1px");
rejects("border-width", "1");
accepts("border-style", "solid dashed");
rejects("border-style", "ridge");
accepts("border-radius", "8px", "8px");
accepts("border-radius", "0", "0");
accepts("border-radius", "0px", "0");
accepts("border-radius", "999px", "999px");
rejects("border-radius", "9999px"); // 上限 1000px;想要药丸形用 50% 或 999px
rejects("border-radius", "1001px");
accepts("border-radius", "50%", "50%");
accepts("border-radius", "0.5rem 1rem", "0.5rem 1rem");
rejects("border-radius", "8px / 4px");
rejects("border-radius", "-4px");
rejects("border-radius", "101%");
rejects("border-radius", "8vw");
rejects("border-radius", "calc(1px + 2px)");
accepts("border-top-left-radius", "4px 8px");
rejects("border-top-left-radius", "4px 8px 1px");
accepts("box-shadow", "0 8px 20px -8px rgba(0,0,0,.35)", "0 8px 20px -8px rgb(0 0 0 / 0.35)");
accepts("box-shadow", "inset 0 1px 0 #fff, 0 0 0 1px #000");
accepts("box-shadow", "none", "none");
rejects("box-shadow", "0 0 0 1px red");
rejects("box-shadow", "0 0 0 1px url(x)");
accepts("box-shadow", "0 0", "0 0"); // 两个偏移量就是合法的 box-shadow
rejects("box-shadow", "0");
rejects("box-shadow", "0 0 0 0 0 #fff");
rejects("box-shadow", "0 0 200px #fff"); // 单个长度上限 128px
rejects("box-shadow", "0 0 0 1px hsl(var(--primary))"); // 阴影颜色必须是字面量
accepts("text-shadow", "0 1px 2px rgba(0,0,0,.5)");
rejects("text-shadow", "inset 0 1px 2px #000");
rejects("text-shadow", "0 1px 2px 3px #000");
accepts("opacity", "0.5", "0.5");
accepts("opacity", "50%", "0.5");
accepts("opacity", "1", "1");
accepts("opacity", "0", "0");
rejects("opacity", "1.5");
rejects("opacity", "-0.1");
rejects("opacity", "150%");
rejects("opacity", "0.5 0.5");
accepts("font-weight", "600", "600");
accepts("font-weight", "bold", "bold");
rejects("font-weight", "650.5");
rejects("font-weight", "0");
rejects("font-weight", "1001");
rejects("font-weight", "heavy");
accepts("font-style", "italic", "italic");
rejects("font-style", "oblique 10deg");
accepts("font-family", '"Inter",  sans-serif', '"Inter", sans-serif');
accepts("font-family", "Charter, serif");
rejects("font-family", 'url("x")');
rejects("font-family", '"Inter"; src: url(x)');
rejects("font-family", '"Inter" sans-serif"');
rejects("font-family", "@import");
accepts("letter-spacing", "0.02em", "0.02em");
accepts("letter-spacing", "-0.5px", "-0.5px");
accepts("letter-spacing", "normal", "normal");
rejects("letter-spacing", "1em");
rejects("letter-spacing", "9px");
rejects("letter-spacing", "0.1");
accepts("text-transform", "uppercase", "uppercase");
rejects("text-transform", "full-width");
accepts("text-decoration-line", "underline line-through", "underline line-through");
accepts("text-decoration-line", "none", "none");
rejects("text-decoration-line", "underline underline");
rejects("text-decoration-line", "blink");
accepts("text-decoration-style", "wavy");
accepts("text-decoration-color", "#f00", "#ff0000");
accepts("backdrop-filter", "blur(8px)", "blur(8px)");
accepts("backdrop-filter", "blur(8px) saturate(1.5)", "blur(8px) saturate(1.5)");
accepts("backdrop-filter", "saturate(180%) brightness(0.9) contrast(1.1)");
accepts("backdrop-filter", "none", "none");
rejects("backdrop-filter", "blur(80px)");
rejects("backdrop-filter", "blur(8)");
rejects("backdrop-filter", "url(#svg-filter)");
rejects("backdrop-filter", "drop-shadow(0 0 2px #000)");
rejects("backdrop-filter", "invert(1)");
rejects("backdrop-filter", "saturate(5)");
rejects("backdrop-filter", "blur(1px) blur(1px) blur(1px) blur(1px)");
accepts("background-clip", "text", "text");
rejects("background-clip", "url(x)");
accepts("outline-color", "hsl(var(--ring))");

// 字号 / 行高:只接受 ±20% 的倍率(Q15)
accepts("font-size", "1.1em", "1.1em");
accepts("font-size", "0.8em", "0.8em");
accepts("font-size", "1.2em", "1.2em");
accepts("font-size", "1em", "1em");
accepts("font-size", ".9em", "0.9em");
accepts("font-size", "1.15EM", "1.15em");
accepts("font-size", "+1.1em", "1.1em");
accepts("font-size", "80%", "80%");
accepts("font-size", "120%", "120%");
accepts("font-size", "105.5%", "105.5%");
for (const bad of [
  "1.21em", "0.79em", "1.2001em", "121%", "79%", "0", "0em", "-1em", "-1.1em", "1.1", "14px", "1rem", "1.1rem",
  "1.1vw", "1.1ex", "1.1ch", "1.1lh", "1.1rlh", "1.1cap", "calc(1em * 1.1)", "calc(1.1em)", "min(1.1em, 20px)",
  "max(1em, 1.1em)", "clamp(1em, 1.1em, 1.2em)", "var(--font-size)", "var(--otr-font-scale)", "larger", "smaller",
  "medium", "x-large", "inherit", "initial", "unset", "revert", "1.1em 1.1em", "1.1em, 1em", "1.1em/1.5", "1.1 em",
  "1.1em !important", "1.1em;", "1.1em; font-size: 5em", "1.1em } body { font-size: 5em", "10em", "1e1em", "1.1e0em",
  "attr(data-size em)", "env(x)",
]) {
  rejects("font-size", bad);
}
accepts("line-height", "1", "1");
accepts("line-height", "0.8", "0.8");
accepts("line-height", "1.2", "1.2");
accepts("line-height", "1.15", "1.15");
accepts("line-height", "90%", "90%");
accepts("line-height", "120%", "120%");
for (const bad of [
  "1.5", "0.5", "2", "1.21", "0.79", "121%", "0", "-1", "20px", "1rem", "1.1em", "normal", "calc(1.1)", "var(--x)",
  "1 1", "1.1;", "1.1 !important", "inherit", "1.1px",
]) {
  rejects("line-height", bad);
}
{
  const r = normalizeDeclaration("font-size", "14px");
  check("font-size rejection explains the rule", !r.ok && r.reason.includes("0.8em–1.2em"), r.ok ? "" : r.reason);
}

// 通用注入尝试:对每个白名单属性都试一遍
const INJECTIONS = [
  "#fff; position: fixed",
  "#fff } * { display: none }",
  "#fff !important",
  "#fff/**/",
  "url(javascript:alert(1))",
  "expression(alert(1))",
  "image-set('x.png' 1x)",
  "@import 'x.css'",
  "#fff</style><script>alert(1)</script>",
  "#fff\\0027",
  "-moz-element(#x)",
  "attr(x)",
  "#fff\u202e",
  "x".repeat(300),
];
for (const prop of CSS_PROPERTIES) {
  for (const inj of INJECTIONS) rejects(prop, inj);
}

// ---------------------------------------------------------------------------
// 7. 键(钩子[:状态])与选择器
// ---------------------------------------------------------------------------
check("parts include core hooks", ["app", "header", "card", "stat-card", "agent-card", "chart", "tooltip", "table", "button", "segmented", "badge", "progress", "settings-section", "switch", "input"].every((p) => THEME_PARTS.includes(p)));
check("states", THEME_STATES.join(",") === "hover,active,focus,disabled,selected");
check("selector card", selectorForKey("card") === '[data-theme-part~="card"]');
check("selector card:hover", selectorForKey("card:hover") === '[data-theme-part~="card"]:hover');
check("selector card:focus", selectorForKey("card:focus") === '[data-theme-part~="card"]:focus-visible');
check("selector card:selected", selectorForKey("card:selected") === '[data-theme-part~="card"][data-theme-state~="selected"]');
check("selector segmented-button:disabled", selectorForKey("segmented-button:disabled") === '[data-theme-part~="segmented-button"]:disabled');
for (const bad of [
  "div", "body", "*", "html", ":root", "card, body", "card body", "card>x", "card:visited", "card:hover:focus",
  "card:", ":hover", "card:nth-child(1)", "card:not(.x)", "card:hover{", "card]", "card\"", "Card", "card ",
  " card", "card:HOVER", "[data-theme-part]", "card:hover,body", "card:has(x)", "card::before", "__proto__",
  "constructor", "", "-card", "card--x:hover:hover",
]) {
  check(`selector rejects ${JSON.stringify(bad)}`, selectorForKey(bad) === null);
}
for (const part of THEME_PARTS) {
  check(`part name shape ${part}`, /^[a-z][a-z0-9-]*$/.test(part) && selectorForKey(part) !== null);
}

// ---------------------------------------------------------------------------
// 8. validateCss:结构、诊断路径、限额、大小写归一化
// ---------------------------------------------------------------------------
function run(raw, path = "tokens.css") {
  const warns = [];
  const out = validateCss(raw, path, (p, m) => warns.push({ path: p, message: m }));
  return { out, warns };
}
{
  const { out, warns } = run({
    card: { "background-image": "linear-gradient(#fff, #000)", position: "fixed", color: "red", " Color ": "#000" },
    "card:hover": { "box-shadow": "0 0 0 1px #fff" },
    "card:visited": { color: "#fff" },
    body: { display: "none" },
    "card:selected": "not an object",
    badge: { color: "#fff; }" },
  });
  check("validateCss keeps valid decls", out?.card?.["background-image"] === "linear-gradient(#ffffff, #000000)");
  check("validateCss normalizes prop name case/space", out?.card?.color === "#000000");
  check("validateCss keeps hover block", out?.["card:hover"]?.["box-shadow"] === "0 0 0 1px rgb(255 255 255 / 1)");
  check("validateCss drops unknown state", out?.["card:visited"] === undefined);
  check("validateCss drops unknown part", out?.body === undefined);
  check("validateCss drops non-object block", out?.["card:selected"] === undefined);
  check("validateCss drops block that ends up empty", out?.badge === undefined);
  const paths = warns.map((w) => w.path);
  check("warn path for bad prop", paths.includes("tokens.css.card.position"), paths.join(" | "));
  check("warn path for bad value", paths.includes("tokens.css.card.color"));
  check("warn path for bad state", paths.includes("tokens.css.card:visited"));
  check("warn path for unknown part", paths.includes("tokens.css.body"));
  check("warn path for non-object block", paths.includes("tokens.css.card:selected"));
  check("warn path for injected value", paths.includes("tokens.css.badge.color"));
  check("warn count", warns.length === 6, String(warns.length));
  check("warn message hints for position", warns.find((w) => w.path.endsWith(".position"))?.message.includes("不支持的属性"));
}
{
  const { out, warns } = run("card { color: red }");
  check("validateCss rejects string", out === undefined && warns.length === 1);
}
{
  const { out, warns } = run(["card"]);
  check("validateCss rejects array", out === undefined && warns.length === 1);
}
{
  const { out } = run(undefined);
  check("validateCss undefined passthrough", out === undefined);
}
{
  const { out, warns } = run({});
  check("validateCss empty object → empty table", out !== undefined && Object.keys(out).length === 0 && warns.length === 0);
}
{
  const raw = {};
  for (const p of THEME_PARTS) {
    raw[p] = { color: "#fff" };
    raw[`${p}:hover`] = { color: "#000" };
    raw[`${p}:active`] = { color: "#000" };
  }
  const { out, warns } = run(raw);
  check("validateCss caps keys at 64", Object.keys(out).length === 64 && warns.some((w) => w.message.includes("64")));
}
{
  const many = {};
  CSS_PROPERTIES.slice(0, 26).forEach((p) => {
    many[p] = "#fff";
  });
  const { warns } = run({ card: many });
  check("validateCss caps decls at 24", warns.some((w) => w.message.includes("24")));
}
{
  const { out, warns } = run({ card: { color: "#" + "f".repeat(300) } });
  check("validateCss caps value length", out?.card === undefined && warns.length === 1);
}
{
  // 原型污染尝试:键不在钩子目录里,直接丢弃
  const raw = JSON.parse('{"__proto__": {"color": "#fff"}, "constructor": {"color": "#fff"}, "card": {"__proto__": "#fff", "color": "#fff"}}');
  const { out } = run(raw);
  check("validateCss ignores __proto__ keys", Object.keys(out).length === 1 && Object.keys(out.card).length === 1 && ({}).color === undefined);
}

// ---------------------------------------------------------------------------
// 9. 合并 / 序列化 / 缓存清洗
// ---------------------------------------------------------------------------
{
  const merged = mergeCss(
    { card: { color: "#000000", opacity: "0.9" }, badge: { color: "#111111" } },
    { card: { color: "#ffffff" }, "card:hover": { opacity: "1" } },
  );
  check("mergeCss per-property", merged.card.color === "#ffffff" && merged.card.opacity === "0.9");
  check("mergeCss keeps untouched key", merged.badge.color === "#111111");
  check("mergeCss adds new key", merged["card:hover"].opacity === "1");
  check("mergeCss undefined passthrough", mergeCss(undefined, undefined) === undefined && mergeCss({ a: {} }, undefined)?.a !== undefined);
}
{
  const text = serializeThemeCss({
    card: { "background-image": "linear-gradient(#ffffff, #000000)", "backdrop-filter": "blur(8px)", "background-clip": "text" },
    "card:hover": { "box-shadow": "var(--shadow-lg)" },
    "agent-card:selected": { "border-color": "hsl(var(--primary) / 0.6)" },
  });
  const expected = [
    '[data-theme-part~="card"] {',
    "  background-image: linear-gradient(#ffffff, #000000);",
    "  -webkit-backdrop-filter: blur(8px);",
    "  backdrop-filter: blur(8px);",
    "  -webkit-background-clip: text;",
    "  background-clip: text;",
    "}",
    '[data-theme-part~="card"]:hover {',
    "  box-shadow: var(--shadow-lg);",
    "}",
    '[data-theme-part~="agent-card"][data-theme-state~="selected"] {',
    "  border-color: hsl(var(--primary) / 0.6);",
    "}",
  ].join("\n");
  check("serialize shape", text === expected, JSON.stringify(text));
  check("serialize empty", serializeThemeCss(undefined) === "" && serializeThemeCss({}) === "");
  // 序列化对「被篡改的表」也不会输出结构字符
  const tampered = serializeThemeCss({
    "card, body": { color: "#fff" },
    card: { position: "fixed", color: "#fff; } body { display: none", "background-image": "url(x)", opacity: "0.5" },
    "__proto__": { color: "#fff" },
  });
  check("serialize drops tampered entries", tampered === '[data-theme-part~="card"] {\n  opacity: 0.5;\n}', JSON.stringify(tampered));
  const lines = text.split("\n");
  check("serialize lines are well-formed", lines.every((l) => /^(\[data-theme-part~="[a-z0-9-]+"\](:[a-z-]+|\[data-theme-state~="selected"\])? \{|  [a-z-]+: [^;{}<>!@\\]*;|\})$/.test(l)));
}
{
  // font-size / line-height 不按字面输出,写成钩子上的倍率变量(Q15)
  const text = serializeThemeCss({
    "card-title": { "font-size": "1.1em", "line-height": "90%", "letter-spacing": "0.02em" },
    "page-title": { "font-size": "115%" },
  });
  const expected = [
    '[data-theme-part~="card-title"] {',
    "  --otr-font-scale: 1.1;",
    "  --otr-line-scale: 0.9;",
    "  letter-spacing: 0.02em;",
    "}",
    '[data-theme-part~="page-title"] {',
    "  --otr-font-scale: 1.15;",
    "}",
  ].join("\n");
  check("serialize scale props as multiplier variables", text === expected, JSON.stringify(text));
  check("serialize never writes font-size / line-height literally", !/(^|\s)(font-size|line-height):/m.test(text));
  const tampered = serializeThemeCss({ card: { "font-size": "3em", "line-height": "2", opacity: "0.5" }, badge: { "font-size": "14px" } });
  check("serialize drops out-of-range scale", tampered === '[data-theme-part~="card"] {\n  opacity: 0.5;\n}', JSON.stringify(tampered));
}
{
  const cleaned = sanitizeCss({
    card: { color: "#ffffff", position: "fixed" },
    body: { color: "#fff" },
    "card:hover": { color: "url(x)" },
  });
  check("sanitizeCss cleans tampered cache", Object.keys(cleaned).length === 1 && cleaned.card.color === "#ffffff" && cleaned.card.position === undefined);
  check("sanitizeCss rejects non-object", sanitizeCss("x") === undefined && sanitizeCss(null) === undefined);
}

// ---------------------------------------------------------------------------
// 10. 端到端:validateManifest + resolveTheme
// ---------------------------------------------------------------------------
{
  const { manifest, diagnostics } = validateManifest({
    apiVersion: 1,
    id: "t",
    name: "T",
    tokens: {
      css: {
        card: { "background-image": "linear-gradient(#fff, #000)", color: "#111", position: "fixed" },
        "card-title": { "letter-spacing": "0.02em" },
      },
    },
    modes: {
      dark: { css: { card: { color: "#eee" }, header: { "backdrop-filter": "blur(12px)" } } },
      light: { css: { card: { color: "#222", "box-shadow": "url(x)" } } },
    },
  });
  check("e2e manifest loads", manifest !== null);
  check("e2e diagnostics are warnings only", diagnostics.every((d) => d.level === "warning") && diagnostics.length === 2, JSON.stringify(diagnostics));
  check("e2e diagnostic paths", diagnostics.map((d) => d.path).sort().join("|") === "modes.light.css.card.box-shadow|tokens.css.card.position");
  const dark = resolveTheme(manifest, "dark", "user");
  const light = resolveTheme(manifest, "light", "user");
  check("e2e dark merge", dark.css.card.color === "#eeeeee" && dark.css.card["background-image"] === "linear-gradient(#ffffff, #000000)" && dark.css.header["backdrop-filter"] === "blur(12px)");
  check("e2e light merge", light.css.card.color === "#222222" && light.css.header === undefined && light.css["card-title"]["letter-spacing"] === "0.02em");
  check("e2e serialize dark", serializeThemeCss(dark.css).includes('[data-theme-part~="header"]'));
  // 老主题(没有 css)→ 空表、空文本;内置主题同样
  const { manifest: plain } = validateManifest({ apiVersion: 1, id: "p", name: "P", modes: { dark: {} } });
  const r = resolveTheme(plain, "dark", "user");
  check("e2e no css → empty", Object.keys(r.css).length === 0 && serializeThemeCss(r.css) === "");
  const builtin = resolveTheme(T.OTR_THEME, "light", "builtin");
  check("builtin has no css", Object.keys(builtin.css).length === 0);
}

// ---------------------------------------------------------------------------
// 10b. 后加 token 的回退链(Q10 / Q11):先回退主题自己写了的「原先那个 token」,再回退默认主题
// ---------------------------------------------------------------------------
{
  const vars = (raw, mode) => {
    const { manifest, diagnostics } = validateManifest({ apiVersion: 1, id: "t", name: "T", ...raw });
    check(`fallback fixture valid ${JSON.stringify(raw).slice(0, 60)}`, manifest && diagnostics.length === 0, JSON.stringify(diagnostics));
    return resolveTheme(manifest, mode, "user").cssVars;
  };
  const builtin = (mode) => resolveTheme(OTR_THEME, mode, "builtin").cssVars;
  check("new tokens are in the catalog", ["successLabel", "warningLabel", "dangerLabel", "switchThumb", "switchThumbOff"].every((k) => COLOR_TOKENS.includes(k)));
  check("every fallback chain points at catalog tokens", Object.entries(COLOR_FALLBACKS).every(([k, chain]) => COLOR_TOKENS.includes(k) && chain.every((c) => COLOR_TOKENS.includes(c))));
  // 1) 老主题:写了 success / warning / dangerText / primaryForeground,没写新 token → 与之前逐位相同
  const old = vars({ modes: { light: { colors: { success: "#2e7d32", warning: "#8a5a00", dangerText: "#a01010", primaryForeground: "#111111" } } } }, "light");
  check("old theme: successLabel ← success", old["--success-label"] === old["--success"]);
  check("old theme: warningLabel ← warning", old["--warning-label"] === old["--warning"]);
  check("old theme: dangerLabel ← dangerText", old["--danger-label"] === old["--danger-text"]);
  check("old theme: switchThumb ← primaryForeground", old["--switch-thumb"] === old["--primary-foreground"]);
  check("old theme: switchThumbOff does NOT follow primaryForeground (Q11 fix) → default", old["--switch-thumb-off"] === builtin("light")["--switch-thumb-off"], old["--switch-thumb-off"]);
  // 旧版霓虹夜那样的暗色主题(深色 primaryForeground):「开」态沿用它,「关」态自动变成默认的白色
  const neonOld = vars({ modes: { dark: { colors: { primaryForeground: "#041017", success: "#2ef08a" } } } }, "dark");
  check("old dark theme: switchThumb ← dark primaryForeground", neonOld["--switch-thumb"] === neonOld["--primary-foreground"]);
  check("old dark theme: switchThumbOff = default white", neonOld["--switch-thumb-off"] === "0 0% 100%", neonOld["--switch-thumb-off"]);
  // 2) 没碰这些颜色的主题 → 默认主题为新 token 调的值
  for (const mode of ["dark", "light"]) {
    const plain = vars({ modes: { [mode]: { colors: { background: mode === "dark" ? "#101418" : "#fafafa" } } } }, mode);
    const b = builtin(mode);
    for (const k of ["--success-label", "--warning-label", "--danger-label", "--switch-thumb", "--switch-thumb-off"]) {
      check(`untouched theme (${mode}) ${k} = default`, plain[k] === b[k], `${plain[k]} vs ${b[k]}`);
    }
  }
  // 3) 公共 tokens 里写的也算「主题自己写了」;模式里写的新 token 优先
  const common = vars({ tokens: { colors: { success: "#2e7d32" } }, modes: { dark: {} } }, "dark");
  check("common success feeds successLabel", common["--success-label"] === common["--success"]);
  const own = vars({ tokens: { colors: { success: "#2e7d32" } }, modes: { dark: { colors: { successLabel: "#7dffb6" } } } }, "dark");
  check("own successLabel wins", own["--success-label"] === "146.3 100% 74.5%" && own["--success-label"] !== own["--success"], own["--success-label"]);
  // 4) switchThumbOff:只写了一个滑块色(switchThumb)时两态同色
  const thumbOnly = vars({ modes: { dark: { colors: { switchThumb: "#eeeeee", primaryForeground: "#000000" } } } }, "dark");
  check("switchThumbOff ← switchThumb", thumbOnly["--switch-thumb-off"] === thumbOnly["--switch-thumb"]);
  const both = vars({ modes: { dark: { colors: { switchThumb: "#000000", switchThumbOff: "#ffffff" } } } }, "dark");
  check("switchThumbOff explicit", both["--switch-thumb-off"] === "0 0% 100%" && both["--switch-thumb"] === "0 0% 0%");
  // 5) 默认主题的新文字 token 在两种模式下对卡片 ≥ 4.5:1(Q10 要求)
  const lum = ({ r, g, b }) => {
    const f = (v) => ((v /= 255) <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
  };
  const contrast = (a, b) => {
    const [x, y] = [lum(parseColor(a)), lum(parseColor(b))];
    return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
  };
  for (const mode of ["dark", "light"]) {
    const b = builtin(mode);
    for (const k of ["--success-label", "--warning-label", "--danger-label"]) {
      const v = contrast(b[k], b["--card"]);
      check(`builtin ${mode} ${k} ≥ 4.5:1 on card`, v >= 4.5, v.toFixed(2));
    }
  }
}

// ---------------------------------------------------------------------------
// 10c. 偏好模式(Q14):单模式主题只改变生效模式,偏好原样带回
// ---------------------------------------------------------------------------
{
  const entry = (m, modes, source = "user") => ({ id: m.id, name: m.name, source, path: null, manifest: m, diagnostics: [], modes });
  const { manifest: lightOnly } = validateManifest({ apiVersion: 1, id: "paper", name: "Paper", modes: { light: {} } });
  const entries = [entry(OTR_THEME, ["dark", "light"], "builtin"), entry(lightOnly, ["light"])];
  const a = decide(entries, "paper", "dark").outcome;
  check("single-mode theme: effective mode switches", a.mode === "light" && a.id === "paper");
  check("single-mode theme: preference kept", a.preferredMode === "dark");
  check("single-mode theme: reason mentions it", typeof a.fallbackReason === "string" && a.fallbackReason.includes("没有暗色"));
  const back = decide(entries, "otr", a.preferredMode).outcome;
  check("back to dual-mode theme: preference restored", back.mode === "dark" && back.preferredMode === "dark" && back.fallbackReason === null);
  const missing = decide(entries, "gone", "light").outcome;
  check("missing theme falls back with preference", missing.id === "otr" && missing.mode === "light" && missing.preferredMode === "light");
}

// ---------------------------------------------------------------------------
// 10d. 字号 / 行高倍率的落地(Q15):tailwind 的每一档都乘倍率变量;组件里没有绕开它的任意字号
// ---------------------------------------------------------------------------
{
  const tw = (await import(pathToFileURL(path.join(repoRoot, "tailwind.config.js")).href)).default;
  const fsz = tw.theme?.extend?.fontSize ?? {};
  const lhs = tw.theme?.extend?.lineHeight ?? {};
  const sizeOf = (v) => (Array.isArray(v) ? v[0] : v);
  check("tailwind fontSize overrides the default scale", ["xs", "sm", "base", "lg", "xl", "2xl", "3xl", "10px", "11px"].every((k) => k in fsz));
  for (const [k, v] of Object.entries(fsz)) {
    check(`tailwind text-${k} multiplies --otr-font-scale`, /^calc\([0-9.]+rem \* var\(--otr-font-scale, 1\)\)$/.test(sizeOf(v)), sizeOf(v));
    if (Array.isArray(v)) check(`tailwind text-${k} line-height multiplies --otr-line-scale`, /var\(--otr-line-scale, 1\)/.test(v[1]), v[1]);
  }
  for (const [k, v] of Object.entries(lhs)) {
    check(`tailwind leading-${k} multiplies --otr-line-scale`, /^calc\([0-9.]+(rem)? \* var\(--otr-line-scale, 1\)\)$/.test(v), v);
  }
  check("tailwind lineHeight covers the default keys", ["none", "tight", "snug", "normal", "relaxed", "loose", "3", "4", "5", "6", "7", "8", "9", "10"].every((k) => k in lhs));
  // 组件里的任意字号 / 行高(text-[12px]、leading-[1.1])不会乘倍率,±20% 的保证就漏了
  const offenders = [];
  const walk = (dir) => {
    for (const ent of fs.readdirSync(dir, { withFileTypes: true })) {
      const p = path.join(dir, ent.name);
      if (ent.isDirectory()) walk(p);
      else if (/\.(tsx?|css|html)$/.test(ent.name)) {
        const src = fs.readFileSync(p, "utf8");
        for (const m of src.matchAll(/\b(text-\[(?:length:)?[0-9.]+[a-z%]*\]|leading-\[[^\]]*\])/g)) offenders.push(`${path.relative(repoRoot, p)}: ${m[1]}`);
        if (/font-size\s*:/.test(src) && !p.endsWith(path.join("theme", "css.ts"))) offenders.push(`${path.relative(repoRoot, p)}: font-size`);
      }
    }
  };
  walk(path.join(repoRoot, "src"));
  check("no arbitrary font-size / line-height outside the scaled scale", offenders.length === 0, offenders.join(" | "));
  // 端到端:清单 → 解析 → 文本里只出现倍率变量
  const { manifest, diagnostics } = validateManifest({
    apiVersion: 1, id: "fs", name: "FS", modes: { dark: { css: { "card-title": { "font-size": "1.1em" }, badge: { "font-size": "20px", "line-height": "1.1" }, table: { "line-height": "1.5" } } } },
  });
  const r = resolveTheme(manifest, "dark", "user");
  const cssText = serializeThemeCss(r.css);
  check("e2e font-size scale", cssText.includes("--otr-font-scale: 1.1;") && cssText.includes("--otr-line-scale: 1.1;") && !cssText.includes("20px"), cssText);
  check("e2e font-size diagnostics", diagnostics.map((d) => d.path).sort().join("|") === "modes.dark.css.badge.font-size|modes.dark.css.table.line-height", JSON.stringify(diagnostics));
}

// ---------------------------------------------------------------------------
// 10e. index.css 的兜底变量与内置默认主题逐位一致(改一处必须改另一处)
// ---------------------------------------------------------------------------
{
  const css = fs.readFileSync(path.join(repoRoot, "src", "index.css"), "utf8");
  const block = (sel) => {
    const m = new RegExp(`(^|\\n)${sel.replace(".", "\\.")}\\s*\\{([^}]*)\\}`).exec(css);
    const out = {};
    for (const decl of (m?.[2] ?? "").split(";")) {
      const i = decl.indexOf(":");
      if (i < 0) continue;
      const name = decl.slice(0, i).trim();
      if (name.startsWith("--")) out[name] = decl.slice(i + 1).replace(/\s+/g, " ").trim();
    }
    return out;
  };
  const root = block(":root");
  const dark = { ...root, ...block(".dark") };
  for (const [mode, fallback] of [["light", root], ["dark", dark]]) {
    const vars = resolveTheme(OTR_THEME, mode, "builtin").cssVars;
    const diffs = [];
    for (const [k, v] of Object.entries(vars)) {
      if (k.startsWith("--chart-") || k.startsWith("--agent-")) continue;
      if (fallback[k] !== v) diffs.push(`${k}: index.css=${fallback[k]} builtin=${v}`);
    }
    check(`index.css ${mode} fallback matches builtin theme`, diffs.length === 0, diffs.join(" | "));
  }
}

// ---------------------------------------------------------------------------
// 10f. 托盘「恢复默认主题」(Q16):Rust 的复位脚本与事件名和前端常量一致
// ---------------------------------------------------------------------------
{
  const rs = fs.readFileSync(path.join(repoRoot, "src-tauri", "src", "themes.rs"), "utf8");
  const script = /pub const RESET_SCRIPT: &str = r#"([\s\S]*?)"#;/.exec(rs)?.[1] ?? "";
  const event = /pub const RESET_EVENT: &str = "([^"]*)";/.exec(rs)?.[1];
  check("rust RESET_EVENT matches THEME_RESET_EVENT", event === THEME_RESET_EVENT, `${event} vs ${THEME_RESET_EVENT}`);
  check("rust RESET_SCRIPT clears the frontend cache key", script.includes(`localStorage.removeItem("${THEME_CACHE_KEY}")`));
  check("rust RESET_SCRIPT removes the managed style element", script.includes(`getElementById("${THEME_STYLE_ID}")`));
  check("rust RESET_SCRIPT is plain JS", (() => {
    try {
      new Function(script);
      return true;
    } catch {
      return false;
    }
  })());
}

// ---------------------------------------------------------------------------
// 11. docs/theme.schema.json 与代码保持同步(钩子目录、状态、属性白名单)
// ---------------------------------------------------------------------------
{
  const schema = JSON.parse(fs.readFileSync(path.join(repoRoot, "docs", "theme.schema.json"), "utf8"));
  const cssDef = schema.definitions?.tokens?.properties?.css;
  const keyPattern = cssDef?.propertyNames?.pattern ?? "";
  const keyRe = new RegExp(keyPattern);
  check("schema: css key pattern accepts every part/state", THEME_PARTS.every((p) => keyRe.test(p) && THEME_STATES.every((st) => keyRe.test(`${p}:${st}`))));
  check("schema: css key pattern rejects junk", !keyRe.test("body") && !keyRe.test("card:visited") && !keyRe.test("card, body"));
  const partsInPattern = (keyPattern.match(/^\^\(([^)]*)\)/)?.[1] ?? "").split("|");
  check("schema: parts list identical", partsInPattern.join("|") === THEME_PARTS.join("|"), `schema=${partsInPattern.length} code=${THEME_PARTS.length}`);
  const blockProps = Object.keys(schema.definitions?.cssBlock?.properties ?? {});
  check("schema: property whitelist identical", blockProps.slice().sort().join("|") === CSS_PROPERTIES.slice().sort().join("|"), `schema=${blockProps.length} code=${CSS_PROPERTIES.length}`);
  const colorProps = Object.keys(schema.definitions?.tokens?.properties?.colors?.properties ?? {});
  check("schema: color token list identical", colorProps.join("|") === COLOR_TOKENS.join("|"), `schema=${colorProps.length} code=${COLOR_TOKENS.length}`);
  // font-size / line-height 的 schema 正则与代码的判定一致(编辑器提示不能比应用更松或更严)
  for (const prop of ["font-size", "line-height"]) {
    const re = new RegExp(schema.definitions.cssBlock.properties[prop].pattern);
    const samples = ["1", "1.1", "0.8", "1.2", "1.21", "0.79", "1.1em", "0.8em", "1.2em", "1.25em", "0.75em", ".9em", "1.2EM", "+1.1em", "80%", "120%", "121%", "79%", "100.5%", "14px", "1rem", "normal", "calc(1em)", "1.5", "2", "0", "-1em"];
    const mismatch = samples.filter((v) => re.test(v) !== normalizeDeclaration(prop, v).ok);
    check(`schema: ${prop} pattern agrees with code`, mismatch.length === 0, mismatch.join(" "));
  }
}

// ---------------------------------------------------------------------------
// 12. 全分支审查补充(round 4):清单结构、文本字段、JSON 报错、首帧缓存、schema 限值、更多绕过串
// ---------------------------------------------------------------------------
{
  const base = { apiVersion: 1, id: "t", name: "T" };
  // modes 里不是对象的模式:告警且**不算提供**(以前会被当成 `{}`,「已忽略」的模式反而出现在可选模式里)
  for (const bad of [null, "dark", 1, [], true]) {
    const r = validateManifest({ ...base, modes: { dark: bad } });
    check(`mode ${JSON.stringify(bad)} alone → no modes → error`, r.manifest === null && r.diagnostics.some((d) => d.level === "error" && d.path === "modes"), JSON.stringify(r.diagnostics));
    const r2 = validateManifest({ ...base, modes: { dark: bad, light: {} } });
    check(`mode ${JSON.stringify(bad)} next to a valid one is dropped`, r2.manifest !== null && Object.keys(r2.manifest.modes).join() === "light" && r2.diagnostics.some((d) => d.path === "modes.dark" && d.level === "warning"), JSON.stringify(r2.diagnostics));
  }
  const empty = validateManifest({ ...base, modes: { dark: {} } });
  check("mode {} still means 'provided, all defaults'", empty.manifest !== null && Object.keys(empty.manifest.modes).join() === "dark" && empty.diagnostics.length === 0);

  // 文本字段:C1 控制字符与双向文本控制符(会把「· 内置」「(无效)」之类的后缀搅乱)按控制字符处理
  for (const ch of ["\u0085", "\u009f", "\u061c", "\u200e", "\u200f", "\u202a", "\u202e", "\u2066", "\u2069"]) {
    const n = validateManifest({ ...base, name: `Evil${ch}Theme`, modes: { dark: {} } });
    check(`name with U+${ch.charCodeAt(0).toString(16).padStart(4, "0")} rejected`, n.manifest === null && n.diagnostics.some((d) => d.path === "name"));
    const a = validateManifest({ ...base, author: `x${ch}`, modes: { dark: {} } });
    check(`author with U+${ch.charCodeAt(0).toString(16).padStart(4, "0")} dropped`, a.manifest !== null && a.manifest.author === undefined && a.diagnostics.some((d) => d.path === "author"));
  }
  const cjk = validateManifest({ ...base, name: "青瓷 Celadon · 深浅", author: "作者 🎨", modes: { dark: {} } });
  check("ordinary CJK / emoji names still fine", cjk.manifest?.name === "青瓷 Celadon · 深浅" && cjk.manifest?.author === "作者 🎨" && cjk.diagnostics.length === 0, JSON.stringify(cjk.diagnostics));

  // JSON 语法错误:引擎报错会带原文片段,截断到 160 字符并去掉控制字符
  const prefix = "不是合法 JSON:";
  for (const text of [`{"a": ${"x".repeat(5000)}}`, `{"a": "b\u0001c"}`, `{"a": \u0007\u202e}`, "{", ""]) {
    const r = parseThemeFile(text);
    const msg = r.diagnostics[0]?.message ?? "";
    check(`json error for ${JSON.stringify(text.slice(0, 16))} is bounded`, r.manifest === null && msg.startsWith(prefix) && msg.length <= prefix.length + 161 && !/[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/.test(msg), JSON.stringify(msg).slice(0, 120));
  }
  check("BOM is still accepted", parseThemeFile(`\ufeff${JSON.stringify({ ...base, modes: { dark: {} } })}`).manifest !== null);
}

// 首帧缓存(bootTheme / resetThemeDom):用一个最小的假 DOM 跑应用里的真实代码
{
  const store = new Map();
  const props = new Map();
  const els = new Map();
  const html = {
    classList: { set: new Set(), toggle(c, on) { if (on) this.set.add(c); else this.set.delete(c); } },
    style: { colorScheme: "", setProperty: (k, v) => props.set(k, v), removeProperty: (k) => props.delete(k) },
    dataset: {},
  };
  const prevDoc = globalThis.document;
  const prevLs = globalThis.localStorage;
  globalThis.localStorage = { getItem: (k) => (store.has(k) ? store.get(k) : null), setItem: (k, v) => store.set(k, String(v)), removeItem: (k) => store.delete(k) };
  globalThis.document = {
    documentElement: html,
    getElementById: (id) => els.get(id) ?? null,
    createElement: (tag) => ({ tagName: tag.toUpperCase(), id: "", textContent: "", remove() { els.delete(this.id); } }),
    head: { appendChild: (el) => els.set(el.id, el) },
  };
  try {
    store.set("token-show-theme", "light");
    store.set(THEME_CACHE_KEY, JSON.stringify({
      id: "cached",
      mode: "light",
      vars: {
        "--background": "0 0% 100%",
        "--agent-custom.bot": "#123456",
        "--agent-my_agent": "#654321",
        "--otr-font-scale": "3",
        "--Background": "x",
        "bad key": "x",
        "--card": 42,
      },
      css: {
        header: { color: "#fff" },
        card: { color: "#fff; } body { display: none", position: "fixed", opacity: "0.5" },
        "card, body": { color: "#fff" },
        badge: { "font-size": "3em" },
      },
    }));
    T.bootTheme();
    check("boot: mode from token-show-theme", !html.classList.set.has("dark") && html.style.colorScheme === "light");
    check("boot: cached vars restored", props.get("--background") === "0 0% 100%" && html.dataset.theme === "cached");
    check("boot: agent vars with . and _ restored (ids allow them)", props.get("--agent-custom.bot") === "#123456" && props.get("--agent-my_agent") === "#654321");
    check("boot: --otr-* never restored from cache", !props.has("--otr-font-scale"));
    check("boot: junk keys / non-string values dropped", !props.has("--Background") && !props.has("bad key") && !props.has("--card"));
    const styleText = els.get(THEME_STYLE_ID)?.textContent ?? "";
    check("boot: cached css re-validated", styleText.includes('[data-theme-part~="header"]') && styleText.includes("opacity: 0.5") && !/position|display|body|3em|--otr-font-scale: 3/.test(styleText), JSON.stringify(styleText));
    // 缓存模式与记忆的模式不一致 → 只切模式、不用缓存
    props.clear();
    els.clear();
    store.set("token-show-theme", "dark");
    T.bootTheme();
    check("boot: cache for another mode is ignored", html.classList.set.has("dark") && props.size === 0 && els.size === 0);
    // 应用一个主题再复位:变量、受控 <style>、缓存都清掉
    const { manifest } = validateManifest({ apiVersion: 1, id: "x", name: "X", modes: { dark: { css: { card: { opacity: "0.9" } } } } });
    T.applyResolvedTheme(resolveTheme(manifest, "dark", "user"));
    check("apply: vars + style + cache written", props.size > 40 && els.has(THEME_STYLE_ID) && store.has(THEME_CACHE_KEY) && html.dataset.theme === "x");
    T.resetThemeDom();
    check("reset: everything the theme wrote is gone", props.size === 0 && !els.has(THEME_STYLE_ID) && !store.has(THEME_CACHE_KEY) && html.dataset.theme === undefined);
    // 被篡改的缓存(不是 JSON / 缺字段)不会抛异常
    for (const junk of ["{", "null", '{"id":1}', '{"id":"a","mode":"sepia","vars":{}}', '{"id":"a","mode":"dark","vars":null}']) {
      store.set(THEME_CACHE_KEY, junk);
      let ok = true;
      try { T.bootTheme(); } catch { ok = false; }
      check(`boot survives junk cache ${junk}`, ok && props.size === 0);
    }
  } finally {
    globalThis.document = prevDoc;
    globalThis.localStorage = prevLs;
  }
}

// schema 与代码:其余 token 组、限值
{
  const schema = JSON.parse(fs.readFileSync(path.join(repoRoot, "docs", "theme.schema.json"), "utf8"));
  const tok = schema.definitions.tokens.properties;
  for (const [group, list] of [["stat", T.STAT_TOKENS], ["font", T.FONT_TOKENS], ["radius", T.RADIUS_TOKENS], ["shadow", T.SHADOW_TOKENS]]) {
    const keys = Object.keys(tok[group]?.properties ?? {});
    check(`schema: ${group} token list identical`, keys.join("|") === list.join("|"), `schema=${keys.join(",")} code=${list.join(",")}`);
  }
  check("schema: chart keys", Object.keys(tok.chart.properties).join("|") === "palette|agents|agentFallback");
  check("schema: palette length = MAX_PALETTE_LENGTH", schema.definitions.colorList.maxItems === T.MAX_PALETTE_LENGTH && schema.definitions.colorList.minItems === 1);
  check("schema: css key count = MAX_CSS_KEYS", tok.css.maxProperties === T.MAX_CSS_KEYS, String(tok.css.maxProperties));
  check("schema: decls per key = MAX_CSS_DECLS", schema.definitions.cssBlock.maxProperties === T.MAX_CSS_DECLS);
  check("schema: css value length = MAX_CSS_VALUE_LEN", schema.definitions.cssValue.maxLength === T.MAX_CSS_VALUE_LEN);
  check("schema: token groups", Object.keys(tok).join("|") === "colors|stat|chart|font|radius|shadow|css");
  check("schema: top-level id pattern = ID rule", schema.properties.id.pattern === "^[a-z0-9][a-z0-9._-]{0,63}$" && tok.chart.properties.agents.propertyNames.pattern === schema.properties.id.pattern);
  // 走字符串语法的三个属性上限是 200(与 token 相同),不是一般值的 256
  const cb = schema.definitions.cssBlock.properties;
  for (const prop of ["font-family", "box-shadow", "text-shadow"]) {
    check(`schema: ${prop} maxLength 200`, cb[prop]?.maxLength === 200, JSON.stringify(cb[prop]));
  }
  check("code: font-family 200 ok / 201 rejected", normalizeDeclaration("font-family", "a".repeat(200)).ok && !normalizeDeclaration("font-family", "a".repeat(201)).ok);
  // 恰好 len 个字符的合法阴影:n 层 "0 0 1px #000",多出来的字符补在最后一层的 1px 上(1.000px / 01px,值不变)
  const shadowOf = (len) => {
    const n = Math.floor((len + 2) / 14);
    const extra = len - (14 * n - 2);
    const last = extra === 0 ? "1px" : extra === 1 ? "01px" : `1.${"0".repeat(extra - 1)}px`;
    return [...Array(n - 1).fill("0 0 1px #000"), `0 0 ${last} #000`].join(", ");
  };
  const s200 = shadowOf(200);
  const s201 = shadowOf(201);
  check("helper builds exact lengths", s200.length === 200 && s201.length === 201, `${s200.length}/${s201.length}`);
  check("code: box-shadow 200 ok / 201 rejected", normalizeDeclaration("box-shadow", s200).ok && !normalizeDeclaration("box-shadow", s201).ok);
  check("code: text-shadow 200 ok / 201 rejected", normalizeDeclaration("text-shadow", s200).ok && !normalizeDeclaration("text-shadow", s201).ok);
}

// 更多绕过串:应用自己的变量、新色彩语法、URL 变体、数学函数、深层嵌套、Unicode 混淆
{
  for (const [prop, v] of [
    ["color", "hsl(var(--otr-font-scale))"],
    ["color", "var(--otr-font-scale)"],
    ["font-size", "var(--otr-font-scale)"],
    ["line-height", "var(--otr-line-scale)"],
    ["opacity", "var(--otr-font-scale)"],
    ["color", "var(--agent-foo_bar)"],
    ["color", "rgb(from #fff r g b)"],
    ["color", "oklch(0.7 0.1 200)"],
    ["color", "lch(50% 30 200)"],
    ["color", "hwb(200 10% 10%)"],
    ["color", "color(srgb 1 0 0)"],
    ["color", "light-dark(#fff, #000)"],
    ["color", "Url(x)"],
    ["color", "URL(x)"],
    ["background-image", "src(x)"],
    ["background-image", "image(#fff)"],
    ["background-image", "linear-gradient(#fff, #000) , url(x)"],
    ["border-radius", "min(1px, 2px)"],
    ["border-radius", "clamp(1px, 2px, 3px)"],
    ["letter-spacing", "max(1px, 2px)"],
    ["font-size", "min(1.1em, 1.2em)"],
    ["opacity", "1e0"],
    ["font-weight", "1e3"],
    ["background-image", "linear-gradient(1e3deg, #fff, #000)"],
    ["background-image", "linear-gradient(#fff 99999999999999999999px, #000)"],
    ["color", "rgb(".repeat(40) + ")".repeat(40)],
    ["color", "#f\u00a0ff"],
    ["color", "#f\u3000ff"],
    ["color", "＃fff"],
    ["color", "#ｆｆｆ"],
    ["color", "#fff\u200b"],
    ["color", "#f\ufeffff"],
    ["color", "rgb(0\u00a00 0)"],
    ["font-family", "a\u202eb"],
    ["font-family", "a\u2028b"],
    ["font-family", "a\u00a0b"],
    ["font-family", '"a\\"b"'],
    ["font-family", '"</style><script>"'],
    ["font-family", "a; b"],
    ["font-family", "a{b}"],
    ["font-family", "a/*b*/"],
    ["font-family", "a\nb"],
    ["box-shadow", "0 0 4px rgba(0,0,0,.5)</style>"],
    ["box-shadow", "0 0 \u00000\u0000"],
    ["box-shadow", "0 0 4px rgba(0,0,0,0.5;x)"],
    ["text-shadow", "0 0 1px rgb(0 0 0) \;"],
  ]) {
    rejects(prop, v);
  }
  // 首尾的 Unicode 空白(NBSP、全角空格、BOM)按 String.prototype.trim 去掉,输出是重新格式化的值
  accepts("color", "#fff\u00a0", "#ffffff");
  accepts("color", "\u3000#fff", "#ffffff");
  accepts("color", "#fff\ufeff", "#ffffff");
  // 全角 / 其它文字的「字母」只在 font-family 里允许,输出里也原样只是字母
  accepts("font-family", "Ｆｏｎｔ, \"思源黑体\"", 'Ｆｏｎｔ, "思源黑体"');
  // 性能上界:64 个键 × 24 条 × 接近上限的值,整张表校验 + 序列化应当很快
  const big = {};
  const parts = THEME_PARTS.slice(0, 32);
  for (const p of parts) {
    for (const st of ["", ":hover"]) {
      const block = {};
      for (const prop of CSS_PROPERTIES.slice(0, 24)) block[prop] = `linear-gradient(${Array.from({ length: 16 }, () => "#abcdef 10%").join(", ")})`.slice(0, 250);
      big[`${p}${st}`] = block;
    }
  }
  const t0 = performance.now();
  const out = validateCss(big, "tokens.css", () => undefined);
  const text = serializeThemeCss(out);
  const ms = performance.now() - t0;
  check("validate + serialize a maxed-out table in < 1500ms", ms < 1500, `${ms.toFixed(0)}ms`);
  check("maxed-out table output is well-formed", typeof text === "string" && !/[;{}]\s*[;{}]/.test(text.replace(/;\n\}/g, "")));
}

// 示例主题:零诊断,并且写全了文档承诺的 token(README / §12.5 的说法由这里守住)
{
  const dir = path.join(repoRoot, "examples", "themes");
  const files = fs.readdirSync(dir).filter((f) => f.endsWith(".json")).sort();
  check("examples: at least five themes", files.length >= 5, files.join(","));
  let dual = 0;
  for (const f of files) {
    const reserved = (T.BUILTIN_THEMES ?? []).map((t) => t.id).filter((id) => `${id}.json` !== f);
    const { manifest, diagnostics } = parseThemeFile(fs.readFileSync(path.join(dir, f), "utf8"), {
      reservedIds: reserved,
    });
    check(`example ${f}: loads with zero diagnostics`, manifest !== null && diagnostics.length === 0, JSON.stringify(diagnostics));
    if (!manifest) continue;
    check(`example ${f}: id matches file name`, `${manifest.id}.json` === f);
    const modes = Object.keys(manifest.modes);
    if (modes.length === 2) dual++;
    for (const mode of modes) {
      const own = (g) => ({ ...manifest.tokens?.[g], ...manifest.modes[mode]?.[g] });
      const miss = [
        ...T.COLOR_TOKENS.filter((k) => own("colors")[k] === undefined).map((k) => `colors.${k}`),
        ...T.STAT_TOKENS.filter((k) => own("stat")[k] === undefined).map((k) => `stat.${k}`),
        ...["palette", "agents", "agentFallback"].filter((k) => own("chart")[k] === undefined).map((k) => `chart.${k}`),
        ...T.FONT_TOKENS.filter((k) => own("font")[k] === undefined).map((k) => `font.${k}`),
        ...T.RADIUS_TOKENS.filter((k) => own("radius")[k] === undefined).map((k) => `radius.${k}`),
        ...T.SHADOW_TOKENS.filter((k) => own("shadow")[k] === undefined).map((k) => `shadow.${k}`),
      ];
      check(`example ${f} (${mode}): complete token set`, miss.length === 0, miss.join(" "));
      const agents = own("chart").agents ?? {};
      check(`example ${f} (${mode}): brand colors for every built-in agent`, ["dsh", "claude-code", "codex", "zcode", "opencode", "pi", "cursor"].every((a) => agents[a]));
    }
  }
  check("examples: at least one dual-mode theme", dual >= 1);
}

// ---------------------------------------------------------------------------
console.log(`theme-css-selftest: ${pass} 通过,${fail} 失败`);
for (const f of failures) console.log(`  ✗ ${f}`);
process.exit(fail > 0 ? 1 : 0);
