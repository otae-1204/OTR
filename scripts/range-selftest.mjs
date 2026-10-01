/**
 * 周期对比的日期窗口、差额文案、按天错位对齐。
 * 用 esbuild 把 src/lib/range.ts 打成临时模块再断言,不引入测试框架。
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
  entryPoints: ["src/lib/range.ts"],
  bundle: true,
  format: "esm",
  platform: "node",
  write: false,
});
const dir = await mkdtemp(join(tmpdir(), "otr-range-"));
const file = join(dir, "range.mjs");
await writeFile(file, built.outputFiles[0].text);
const range = await import(pathToFileURL(file).href);

let failed = 0;
function check(name, actual, expected) {
  const ok = JSON.stringify(actual) === JSON.stringify(expected);
  if (!ok) {
    failed += 1;
    console.error(`FAIL ${name}\n  actual   ${JSON.stringify(actual)}\n  expected ${JSON.stringify(expected)}`);
  }
}

function span(from, to, all = false) {
  return { from, to, all };
}

check("今日", range.previousRange("today", span("2026-10-01", "2026-10-01")), {
  from: "2026-09-30",
  to: "2026-09-30",
  label: "昨日",
});
check("近7天", range.previousRange("7d", span("2026-09-25", "2026-10-01")), {
  from: "2026-09-18",
  to: "2026-09-24",
  label: "前 7 天",
});
check("近30天", range.previousRange("30d", span("2026-09-02", "2026-10-01")), {
  from: "2026-08-03",
  to: "2026-09-01",
  label: "前 30 天",
});
check("本月 10-01", range.previousRange("month", span("2026-10-01", "2026-10-01")), {
  from: "2026-09-01",
  to: "2026-09-01",
  label: "上月同期",
});
check("本月 10-31", range.previousRange("month", span("2026-10-01", "2026-10-31")), {
  from: "2026-09-01",
  to: "2026-09-30",
  label: "上月同期",
});
check("本月 2026-03-31", range.previousRange("month", span("2026-03-01", "2026-03-31")), {
  from: "2026-02-01",
  to: "2026-02-28",
  label: "上月同期",
});
check("本月 2024-03-31", range.previousRange("month", span("2024-03-01", "2024-03-31")), {
  from: "2024-02-01",
  to: "2024-02-29",
  label: "上月同期",
});
check("全部", range.previousRange("all", span("2000-01-01", "2026-10-01", true)), null);
check("自定义 3 天", range.previousRange("custom", span("2026-09-10", "2026-09-12")), {
  from: "2026-09-07",
  to: "2026-09-09",
  label: "前 3 天",
});
check("自定义单日", range.previousRange("custom", span("2026-09-10", "2026-09-10")), {
  from: "2026-09-09",
  to: "2026-09-09",
  label: "前一天",
});
check("from 晚于 to", range.previousRange("custom", span("2026-09-12", "2026-09-10")), null);

const fullCurrent = [
  "2026-09-25",
  "2026-09-26",
  "2026-09-27",
  "2026-09-28",
  "2026-09-29",
  "2026-09-30",
  "2026-10-01",
];
const fullPrev = [
  "2026-09-18",
  "2026-09-19",
  "2026-09-20",
  "2026-09-21",
  "2026-09-22",
  "2026-09-23",
  "2026-09-24",
];
check(
  "按天错位:可见第 1 天对上一周期第 3 天",
  range.alignPreviousBucket("2026-09-27", fullCurrent, fullPrev),
  "2026-09-20",
);
check(
  "本月多出来的一天没有对应桶",
  range.alignPreviousBucket("d30", Array.from({ length: 31 }, (_, i) => `d${i}`), Array.from({ length: 30 }, (_, i) => `p${i}`)),
  null,
);

const abs = (n) => String(n);
check(
  "上期为 0",
  range.compareText({
    label: "昨日",
    current: 5,
    previous: 0,
    formatAbs: abs,
    previousWindow: "2026-09-30",
  }),
  { text: "昨日无用量", title: "对比 2026-09-30", mark: "无用量" },
);
check(
  "持平",
  range.compareText({
    label: "昨日",
    current: 10,
    previous: 10,
    formatAbs: abs,
    previousWindow: "2026-09-30",
  }),
  { text: "与昨日持平", title: "对比 2026-09-30 · 用量相同", mark: "持平" },
);
check(
  "+25%",
  range.compareText({
    label: "昨日",
    current: 100,
    previous: 80,
    formatAbs: abs,
    previousWindow: "2026-09-30",
  }),
  { text: "较昨日 +25%", title: "对比 2026-09-30 · 多 20", mark: "+25%" },
);
{
  const tiny = range.compareText({
    label: "昨日",
    current: 100.04,
    previous: 100,
    formatAbs: abs,
    previousWindow: "2026-09-30",
  });
  check("四舍五入成 0 的文案", tiny.text, "较昨日 +<0.1%");
  check(
    "四舍五入成 0 仍写出绝对差额",
    tiny.title.startsWith("对比 2026-09-30 · 多 "),
    true,
  );
}
check(
  "成本浮点抖动视为持平",
  range.compareText({
    label: "昨日",
    current: 1.2,
    previous: 1.2 + 1e-9,
    formatAbs: abs,
    previousWindow: "2026-09-30",
    epsilon: 1e-6,
    flatDetail: "金额相同",
  }),
  { text: "与昨日持平", title: "对比 2026-09-30 · 金额相同", mark: "持平" },
);

if (failed > 0) {
  console.error(`${failed} failed`);
  process.exit(1);
}
console.log("range selftest ok");
