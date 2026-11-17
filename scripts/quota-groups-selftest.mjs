/**
 * Agy 额度窗口的分组:池的顺序、缺百分比的隐藏、落单窗口。
 * 用 esbuild 把 src/lib/quotaGroups.ts 打成临时模块再断言。
 */
import { createRequire } from "node:module";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const require = createRequire(import.meta.url);
const esbuild = require("esbuild");
const root = join(dirname(fileURLToPath(import.meta.url)), "..");

const built = await esbuild.build({
  absWorkingDir: root,
  entryPoints: ["src/lib/quotaGroups.ts"],
  bundle: true,
  format: "esm",
  platform: "node",
  write: false,
});
const dir = await mkdtemp(join(tmpdir(), "otr-quota-"));
const file = join(dir, "quotaGroups.mjs");
await writeFile(file, built.outputFiles[0].text);
const quota = await import(pathToFileURL(file).href);

let failed = 0;
function check(name, actual, expected) {
  const ok = JSON.stringify(actual) === JSON.stringify(expected);
  if (!ok) {
    failed += 1;
    console.error(
      `FAIL ${name}\n  actual   ${JSON.stringify(actual)}\n  expected ${JSON.stringify(expected)}`,
    );
  }
}

const w = (partial) => ({ label: partial.key, ...partial });

check(
  "no pool stays flat",
  quota.groupQuotaWindows([w({ key: "weekly", usedPercent: 10 })]),
  null,
);
check(
  "expiry alone is not a pool",
  quota.quotaMeterLayout([w({ key: "plan", resetAt: 1 })]).kind,
  "flat",
);

const grouped = quota.groupQuotaWindows([
  w({ key: "weekly", group: "argon", usedPercent: 40, windowSeconds: 604800 }),
  w({
    key: "five_hour",
    group: "argon",
    usedPercent: null,
    resetAt: 9,
    windowSeconds: 18000,
  }),
  w({ key: "weekly", group: "claude_gpt", usedPercent: 10, windowSeconds: 604800 }),
  w({ key: "five_hour", group: "gemini", usedPercent: 80, windowSeconds: 18000 }),
  w({ key: "weekly", group: "gemini", usedPercent: 20, windowSeconds: 604800 }),
  w({ key: "five_hour", group: "claude_gpt", usedPercent: 0, windowSeconds: 18000 }),
  w({ key: "five_hour", group: "gemini_flash", usedPercent: 5, windowSeconds: 18000 }),
  w({ key: "weekly", group: "sakana", groupLabel: "Sakana", usedPercent: 1, windowSeconds: 604800 }),
  w({ key: "weekly", group: "mimo", usedPercent: 2, windowSeconds: 604800 }),
]);

check(
  "pools stay paired and ordered",
  grouped.map((g) => ({
    id: g.id,
    label: g.label,
    keys: g.windows.map((x) => `${x.key}:${x.usedPercent}`),
  })),
  [
    { id: "gemini", label: "Gemini", keys: ["five_hour:80", "weekly:20"] },
    { id: "gemini_flash", label: "Gemini Flash", keys: ["five_hour:5"] },
    { id: "claude_gpt", label: "Claude / GPT", keys: ["five_hour:0", "weekly:10"] },
    { id: "argon", label: "Gemini 4 Argon", keys: ["weekly:40"] },
    { id: "sakana", label: "Sakana", keys: ["weekly:1"] },
    { id: "mimo", label: "mimo", keys: ["weekly:2"] },
  ],
);

check(
  "alias keys join the same pool",
  quota.groupQuotaWindows([
    w({ key: "gemini_4_argon:weekly", label: "每周", usedPercent: 10, windowSeconds: 604800 }),
    w({ key: "five_hour", label: "5 小时", group: "argon", usedPercent: 20, windowSeconds: 18000 }),
  ]).map((g) => ({ id: g.id, keys: g.windows.map((x) => x.key) })),
  [{ id: "argon", keys: ["five_hour", "gemini_4_argon:weekly"] }],
);

check(
  "ungrouped meters stay after the pools",
  quota.groupQuotaWindows([
    w({ key: "plan", label: "套餐额度", usedPercent: 2 }),
    w({ key: "five_hour", group: "gemini", usedPercent: 1, windowSeconds: 18000 }),
  ]).map((g) => g.id),
  ["gemini", "__rest"],
);

check(
  "a pool with no finite percent is empty, not a fake bar",
  quota.quotaMeterLayout([
    w({ key: "five_hour", group: "gemini", usedPercent: null, resetAt: 5 }),
  ]),
  { kind: "empty" },
);

check(
  "a lone window spans its own row",
  quota.pairQuotaRows(["five_hour", "weekly", "monthly"]),
  [["five_hour", "weekly"], ["monthly"]],
);

if (failed > 0) {
  console.error(`${failed} failed`);
  process.exit(1);
}
console.log("quota groups ok");
