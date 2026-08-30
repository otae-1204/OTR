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
