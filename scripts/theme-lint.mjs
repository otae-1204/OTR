#!/usr/bin/env node
/**
 * 主题文件校验器(给主题作者用)。
 *
 *   node scripts/theme-lint.mjs path/to/theme.json [更多文件...]
 *
 * 用的是应用里同一份校验/解析代码(src/theme/),所以这里通过 = 应用里能加载。
 * 输出:错误/警告列表、每个模式解析后的关键 token。退出码:有 error 为 1,否则 0。
 * 依赖 esbuild(vite 自带),先 `npm install`。
 */
import { build } from "esbuild";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const files = process.argv.slice(2);
if (files.length === 0) {
  console.error("用法:node scripts/theme-lint.mjs <theme.json> [...]");
  process.exit(2);
}

// 把 src/theme 打成一个临时 ESM 模块(放在 node_modules 下,依赖解析才找得到)
const outDir = path.join(root, "node_modules", ".cache", "otr-theme-lint");
fs.mkdirSync(outDir, { recursive: true });
const outfile = path.join(outDir, `theme-${process.pid}.mjs`);
await build({
  entryPoints: [path.join(root, "src", "theme", "index.ts")],
  bundle: true,
  format: "esm",
  platform: "node",
  outfile,
  logLevel: "error",
  external: ["react", "react/jsx-runtime", "@tauri-apps/api/core"],
});
const theme = await import(pathToFileURL(outfile).href);
fs.rmSync(outfile, { force: true });

const { parseThemeFile, resolveTheme, BUILTIN_THEMES, THEME_API_VERSION } = theme;
const reserved = BUILTIN_THEMES.map((t) => t.id);
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
  const { manifest, diagnostics } = parseThemeFile(text, { reservedIds: reserved });
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
  }
  if (diagnostics.some((d) => d.level === "warning")) {
    console.log("  (有警告的 token 会回退到默认主题的值)");
  }
}
process.exit(failed ? 1 : 0);
