import dayjs from "dayjs";

/** 日期范围预设 */
export type RangePreset = "today" | "7d" | "30d" | "month" | "all" | "custom";

export interface DateRange {
  from: string;
  to: string;
  /** 全部时间:会话查询时 from/to 传 null,统计查询用远早于数据的起点 */
  all: boolean;
}

export const PRESET_LABELS: Record<RangePreset, string> = {
  today: "今日",
  "7d": "近7天",
  "30d": "近30天",
  month: "本月",
  all: "全部",
  custom: "自定义",
};

export function todayStr(): string {
  return dayjs().format("YYYY-MM-DD");
}

export function computeRange(
  preset: RangePreset,
  customFrom: string,
  customTo: string,
): DateRange {
  const today = todayStr();
  switch (preset) {
    case "today":
      return { from: today, to: today, all: false };
    case "7d":
      return {
        from: dayjs().subtract(6, "day").format("YYYY-MM-DD"),
        to: today,
        all: false,
      };
    case "month":
      return {
        from: dayjs().startOf("month").format("YYYY-MM-DD"),
        to: today,
        all: false,
      };
    case "all":
      return { from: "2000-01-01", to: today, all: true };
    case "custom":
      return {
        from: customFrom || dayjs().subtract(29, "day").format("YYYY-MM-DD"),
        to: customTo || today,
        all: false,
      };
    case "30d":
    default:
      return {
        from: dayjs().subtract(29, "day").format("YYYY-MM-DD"),
        to: today,
        all: false,
      };
  }
}

/** 展示用标题,如 "近30天" / "2026-08-01 ~ 2026-08-29" */
export function rangeTitle(
  preset: RangePreset,
  range: DateRange,
): string {
  if (preset === "today") return "今日总览";
  if (preset === "custom") return `${range.from} ~ ${range.to}`;
  return PRESET_LABELS[preset];
}

/** 明确统计口径的副标题,避免"为什么是 0"的歧义 */
export function rangeSubtitle(preset: RangePreset, range: DateRange): string {
  if (preset === "today") return `统计 ${range.from} 当天的用量`;
  if (preset === "all") return "统计全部历史用量";
  return `统计 ${range.from} ~ ${range.to} 的用量`;
}

/** 首尾都算的天数。from 晚于 to 时为 0 或负数。 */
export function inclusiveDays(from: string, to: string): number {
  return dayjs(to).startOf("day").diff(dayjs(from).startOf("day"), "day") + 1;
}

export function formatRangeSpan(from: string, to: string): string {
  return from === to ? from : `${from} ~ ${to}`;
}

/** 紧挨当前窗口、同样长度的上一周期。全部时间、以及 from 晚于 to 时没有。 */
export interface PreviousRange {
  from: string;
  to: string;
  /** 短标签:昨日 / 前 7 天 / 上月同期 … */
  label: string;
}

export function previousRange(
  preset: RangePreset,
  range: DateRange,
): PreviousRange | null {
  if (preset === "all" || range.all) return null;
  const n = inclusiveDays(range.from, range.to);
  if (n < 1) return null;

  if (preset === "month") {
    const start = dayjs(range.from).startOf("month");
    const dayIndex = dayjs(range.to).date();
    const prevMonth = start.subtract(1, "month");
    const prevFrom = prevMonth.startOf("month");
    const prevTo = prevFrom.date(Math.min(dayIndex, prevMonth.daysInMonth()));
    return {
      from: prevFrom.format("YYYY-MM-DD"),
      to: prevTo.format("YYYY-MM-DD"),
      label: "上月同期",
    };
  }

  const prevTo = dayjs(range.from).subtract(1, "day");
  const prevFrom = prevTo.subtract(n - 1, "day");
  let label = `前 ${n} 天`;
  if (preset === "today") label = "昨日";
  else if (preset === "7d") label = "前 7 天";
  else if (preset === "30d") label = "前 30 天";
  else if (n === 1) label = "前一天";
  return {
    from: prevFrom.format("YYYY-MM-DD"),
    to: prevTo.format("YYYY-MM-DD"),
    label,
  };
}

export interface CompareText {
  /** 大卡上的一行,如「较昨日 +12.4%」「与昨日持平」「昨日无用量」 */
  text: string;
  /** 悬停:对比区间 + 绝对差额 */
  title: string;
  /** 不带周期名,给窄卡片同一行用:+66.2% / 持平 / 无用量 */
  mark: string;
}

function formatSignedPct(current: number, previous: number): string {
  const pct = ((current - previous) / previous) * 100;
  const rounded = Math.round(pct * 10) / 10;
  if (rounded === 0) return pct > 0 ? "+<0.1%" : "-<0.1%";
  const abs = Math.abs(rounded);
  const body = Number.isInteger(abs) ? String(abs) : abs.toFixed(1);
  return `${rounded > 0 ? "+" : "-"}${body}%`;
}

/**
 * 本期相对上一周期的差额文案。
 * epsilon 用于成本这类浮点:差值不超过它视为持平。token 传 0。
 * 上一周期不超过 epsilon、本期更高时不写百分比。
 */
export function compareText(opts: {
  label: string;
  current: number;
  previous: number;
  formatAbs: (n: number) => string;
  previousWindow: string;
  epsilon?: number;
  /** 持平时悬停里的那半句,成本用「金额相同」 */
  flatDetail?: string;
}): CompareText {
  const eps = opts.epsilon ?? 0;
  const delta = opts.current - opts.previous;
  const window = opts.previousWindow;
  if (Math.abs(delta) <= eps) {
    return {
      text: `与${opts.label}持平`,
      title: `对比 ${window} · ${opts.flatDetail ?? "用量相同"}`,
      mark: "持平",
    };
  }
  if (opts.previous <= eps) {
    return {
      text: `${opts.label}无用量`,
      title: `对比 ${window}`,
      mark: "无用量",
    };
  }
  const pct = formatSignedPct(opts.current, opts.previous);
  const direction = delta > 0 ? "多" : "少";
  return {
    text: `较${opts.label} ${pct}`,
    title: `对比 ${window} · ${direction} ${opts.formatAbs(Math.abs(delta))}`,
    mark: pct,
  };
}

/**
 * 可见桶在完整当前窗口里的序号,对上上一周期同一序号的桶。
 * 按天的横轴会砍掉首尾没数据的日子,不能按「看得见的第 1 天」去对。
 * 序号超出上一周期(本月 31 日对上月 30 日)返回 null,调用方不要补 0。
 */
export function alignPreviousBucket(
  currentBucket: string,
  fullCurrent: readonly string[],
  fullPrevious: readonly string[],
): string | null {
  const idx = fullCurrent.indexOf(currentBucket);
  if (idx < 0 || idx >= fullPrevious.length) return null;
  return fullPrevious[idx];
}
