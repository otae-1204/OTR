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

/** 成本显示:>=0.01 保留 2 位,否则保留 4 位 */
export function fmtCost(
  n: number,
  currency: string = "CNY",
  rate: number = 7.2,
): string {
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
