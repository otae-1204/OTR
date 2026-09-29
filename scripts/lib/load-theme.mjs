/**
 * 把 src/theme 打成一个临时 ESM 模块再导入,让命令行脚本用上应用里同一份校验/解析代码。
 * 依赖 esbuild(vite 自带),先 `npm install`。
 */
import { build } from "esbuild";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");

/** @param {string} tag 缓存目录名,避免多个脚本并行时互相覆盖 */
export async function loadThemeModule(tag) {
  // 放在 node_modules 下,依赖解析才找得到
  const outDir = path.join(repoRoot, "node_modules", ".cache", `otr-${tag}`);
  fs.mkdirSync(outDir, { recursive: true });
  const outfile = path.join(outDir, `theme-${process.pid}.mjs`);
  await build({
    entryPoints: [path.join(repoRoot, "src", "theme", "index.ts")],
    bundle: true,
    format: "esm",
    platform: "node",
    outfile,
    logLevel: "error",
    external: ["react", "react/jsx-runtime", "@tauri-apps/api/core", "@tauri-apps/api/event"],
  });
  try {
    return await import(pathToFileURL(outfile).href);
  } finally {
    fs.rmSync(outfile, { force: true });
  }
}
