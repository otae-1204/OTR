import dayjs from "dayjs";
import relativeTime from "dayjs/plugin/relativeTime";
import "dayjs/locale/zh-cn";

dayjs.extend(relativeTime);
dayjs.locale("zh-cn");

/** 去掉小数末尾多余的 0:"1.20" -> "1.2","1.00" -> "1" */
function trimTrailingZeros(s: string): string {
  if (!s.includes(".")) return s;
  return s.replace(/0+$/, "").replace(/\.$/, "");
}

/** 紧凑 token 数:1234567 -> "1.23M",支持 K/M/B */
export function fmtTokens(n: number): string {
  if (!Number.isFinite(n)) return "--";
  const abs = Math.abs(n);
  if (abs >= 1e9) return `${trimTrailingZeros((n / 1e9).toFixed(2))}B`;
  if (abs >= 1e6) return `${trimTrailingZeros((n / 1e6).toFixed(2))}M`;
  if (abs >= 1e3) return `${trimTrailingZeros((n / 1e3).toFixed(1))}K`;
  return String(Math.round(n));
}

/**
 * 生成速度。没有可信耗时时返回 null。
 * 分子只用带耗时的输出,不能拿全部 output 来除。
 */
export function fmtTps(outputTokens: number, genMs: number): string | null {
  if (!(outputTokens > 0) || !(genMs > 0)) return null;
  const tps = outputTokens / (genMs / 1000);
  if (!Number.isFinite(tps) || tps <= 0) return null;
  if (tps >= 100) return `${Math.round(tps)} token/s`;
  if (tps >= 10) return `${trimTrailingZeros(tps.toFixed(1))} token/s`;
  return `${trimTrailingZeros(tps.toFixed(2))} token/s`;
}

/** 平均首字延迟。样本不足时返回 null。 */
export function fmtTtft(totalMs: number, samples: number): string | null {
  if (!(samples > 0) || !(totalMs > 0)) return null;
  const ms = totalMs / samples;
  if (!Number.isFinite(ms) || ms <= 0) return null;
  if (ms >= 1000) return `${trimTrailingZeros((ms / 1000).toFixed(1))} 秒`;
  return `${Math.round(ms)} ms`;
}

export function tpsTitle(outputTokens: number, genMs: number, samples: number): string {
  const seconds = trimTrailingZeros((genMs / 1000).toFixed(1));
  const calls = samples > 0 ? `${samples.toLocaleString()} 次 · ` : "";
  return `${calls}${outputTokens.toLocaleString()} token / ${seconds} 秒`;
}

/** 成本显示:后端金额统一为 ¥,展示时按全局币种换算。>=0.01 保留 2 位,否则 4 位 */
export function fmtCost(n: number, currency: string, rate: number): string {
  if (currency === "USD") {
    const v = n / rate;
    return `≈$${v.toFixed(Math.abs(v) >= 0.01 || v === 0 ? 2 : 4)}`;
  }
  return `≈¥${n.toFixed(Math.abs(n) >= 0.01 || n === 0 ? 2 : 4)}`;
}

/** 绝对时间(短):MM-DD HH:mm */
export function fmtTime(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return "--";
  return dayjs(ms).format("MM-DD HH:mm");
}

/** 日期:YYYY-MM-DD */
export function fmtDate(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return "--";
  return dayjs(ms).format("YYYY-MM-DD");
}

/** 当天时刻:HH:mm:ss */
export function fmtClock(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return "--";
  return dayjs(ms).format("HH:mm:ss");
}

/** 绝对时间(完整,用于 title 提示) */
export function fmtDateTime(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return "--";
  return dayjs(ms).format("YYYY-MM-DD HH:mm:ss");
}

/** 中文相对时间:"3 分钟前" */
export function fromNow(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return "--";
  return dayjs(ms).fromNow();
}
