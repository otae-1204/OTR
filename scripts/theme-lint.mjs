#!/usr/bin/env node
/**
 * 主题文件校验器(给主题作者用)。
 *
 *   node scripts/theme-lint.mjs path/to/theme.json [更多文件...]
 *   node scripts/theme-lint.mjs --print-css path/to/theme.json   # 额外打印生成的样式文本
 *
 * 用的是应用里同一份校验/解析代码(src/theme/),所以这里通过 = 应用里能加载。
 * 输出:错误/警告列表(含 `css` 字段的逐条诊断)、每个模式解析后的关键 token、
 * 后加的可选 token(状态文字 / 开关滑块)实际取到的值与来源、受限自定义 CSS 的规则数。
 * 退出码:有 error 为 1,否则 0。
 * 依赖 esbuild(vite 自带),先 `npm install`。
 *
 * css 校验器本身的自检用例在 scripts/theme-css-selftest.mjs(`npm run test:theme`)。
 */
import fs from "node:fs";
import { loadThemeModule } from "./lib/load-theme.mjs";

const args = process.argv.slice(2);
const printCss = args.includes("--print-css");
const files = args.filter((a) => a !== "--print-css");
if (files.length === 0) {
  console.error("用法:node scripts/theme-lint.mjs [--print-css] <theme.json> [...]");
  process.exit(2);
}

const theme = await loadThemeModule("theme-lint");
const {
  parseThemeFile,
  resolveTheme,
  serializeThemeCss,
  BUILTIN_THEMES,
  THEME_API_VERSION,
  COLOR_FALLBACKS,
} = theme;
const kebab = (s) => s.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);

/** 后加的可选 token 取到的值与来源:主题自己写的 / 回退自主题的某个 token / 默认主题 */
function fallbackSummary(manifest, mode, vars) {
  const own = { ...manifest.tokens?.colors, ...manifest.modes[mode]?.colors };
  return Object.entries(COLOR_FALLBACKS)
    .map(([token, chain]) => {
      const from = own[token] !== undefined ? null : chain.find((k) => own[k] !== undefined);
      const src = own[token] !== undefined ? "" : from ? `(← ${from})` : "(默认主题)";
      return `${token}=${vars[`--${kebab(token)}`]}${src}`;
    })
    .join(" ");
}
const reserved = BUILTIN_THEMES.map((t) => t.id);
/** 示例目录里与内置主题同名的文件是打包源,不当作用户主题去撞保留 id */
function reservedFor(file) {
  const norm = file.replace(/\\/g, "/");
  const base = norm.split("/").pop()?.replace(/\.json$/, "");
  if (norm.includes("/examples/themes/")) {
    return reserved.filter((id) => id !== base);
  }
  return reserved;
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
  const { manifest, diagnostics } = parseThemeFile(text, { reservedIds: reservedFor(file) });
  for (const d of diagnostics) {
    console.log(`  [${d.level === "error" ? "错误" : "警告"}] ${d.path || "/"}: ${d.message}`);
  }
  if (!manifest) {
    console.log("  ✗ 未通过:应用会拒绝加载这个主题并回退默认主题");
    failed = true;
    continue;
  }
  const modes = Object.keys(manifest.modes);
  console.log(
    `  ✓ 通过:id=${manifest.id} name=${manifest.name} version=${manifest.version ?? "-"} ` +
      `modes=${modes.join(",")} apiVersion=${manifest.apiVersion}(应用支持 v${THEME_API_VERSION})`,
  );
  for (const mode of modes) {
    const r = resolveTheme(manifest, mode, "user");
    const v = r.cssVars;
    console.log(
      `  · ${mode}: background=${v["--background"]} foreground=${v["--foreground"]} ` +
        `primary=${v["--primary"]} card=${v["--card"]} font-sans=${v["--font-sans"]}`,
    );
    console.log(
      `    palette=[${r.chart.palette.join(", ")}] agents=${JSON.stringify(r.chart.agents)}`,
    );
    console.log(`    ${fallbackSummary(manifest, mode, v)}`);
    const keys = Object.keys(r.css);
    const decls = keys.reduce((n, k) => n + Object.keys(r.css[k]).length, 0);
    console.log(`    css: ${keys.length} 个钩子条目,${decls} 条声明`);
    if (printCss && keys.length > 0) {
      const cssText = serializeThemeCss(r.css);
      console.log(cssText.replace(/^/gm, "      "));
    }
  }
  if (diagnostics.some((d) => d.level === "warning")) {
    console.log("  (有警告的 token / css 条目会被丢弃,token 回退到默认主题的值)");
  }
}
process.exit(failed ? 1 : 0);
