/**
 * 把额度窗口收成「模型池 × 周期」的行。
 *
 * Antigravity 一条账号上同时有好几池:Gemini、Claude / GPT、Gemini 4 Argon,
 * 每池通常是 5 小时加每周。平铺进三列会把同一池拆到两行。
 * 有 `group`(或 `池:周期` 这种 key)的来源改成一池一行、短周期在左。
 * 没有池的来源仍返回 null,额度页继续用原来的三列。
 *
 * 池 id 约定,后端按这个填 `group`:
 * gemini、gemini_pro、gemini_flash、claude、claude_gpt、argon。
 * 没有有限用量百分比的窗口整段不画:空条会被看成用完了。0% 仍显示。
 */
import type { QuotaWindow } from "../api/bindings";

export interface QuotaGroup {
  id: string;
  /** 空字符串 = 这一行没有池名,格子按原来的窗口名铺开 */
  label: string;
  windows: QuotaWindow[];
}

export type QuotaMeterLayout =
  | { kind: "flat" }
  | { kind: "empty" }
  | { kind: "groups"; groups: QuotaGroup[] };

const WINDOW_ORDER = [
  "five_hour",
  "weekly",
  "monthly",
  "plan",
  "auto",
  "api",
  "credit",
  "topup",
  "balance",
];

/** 已知池的显示顺序。不认识的池排在后面,并保持第一次出现的先后。 */
const GROUP_ORDER = [
  "gemini",
  "gemini_pro",
  "gemini_flash",
  "claude",
  "claude_gpt",
  "argon",
];

const GROUP_LABELS: Record<string, string> = {
  gemini: "Gemini",
  gemini_pro: "Gemini Pro",
  gemini_flash: "Gemini Flash",
  claude: "Claude",
  claude_gpt: "Claude / GPT",
  argon: "Gemini 4 Argon",
};

const GROUP_ALIASES: Record<string, string> = {
  "claude-gpt": "claude_gpt",
  "claude/gpt": "claude_gpt",
  gemini4: "argon",
  gemini_4: "argon",
  gemini_4_argon: "argon",
  "gemini-4-argon": "argon",
};

function canonicalGroup(raw: string): string {
  const key = raw.trim().toLowerCase().replace(/\s+/g, "_");
  return GROUP_ALIASES[key] ?? key;
}

/** 窗口所属的池。显式 group 优先;否则认 `argon:weekly` 这种 key。 */
export function quotaGroupId(w: QuotaWindow): string | null {
  const explicit = w.group?.trim();
  if (explicit) return canonicalGroup(explicit);
  const colon = w.key.indexOf(":");
  if (colon > 0) {
    const id = w.key.slice(0, colon).trim();
    if (id) return canonicalGroup(id);
  }
  return null;
}

/**
 * 只有日期、没有用量的窗口(套餐有效期)。
 * 已经标了池的窗口不算到期:缺百分比时整段隐藏,不改写成「到期」。
 */
export function isExpiryWindow(w: QuotaWindow): boolean {
  if (quotaGroupId(w)) return false;
  return typeof w.usedPercent !== "number" && w.resetAt != null && w.resetAt !== 0;
}

function metricKey(w: QuotaWindow): string {
  const colon = w.key.lastIndexOf(":");
  if (colon > 0) return w.key.slice(colon + 1);
  return w.key;
}

function windowRank(w: QuotaWindow): [number, number] {
  if (isExpiryWindow(w)) return [2, w.resetAt ?? 0];
  if (typeof w.windowSeconds === "number" && w.windowSeconds > 0) {
    return [0, w.windowSeconds];
  }
  const i = WINDOW_ORDER.indexOf(metricKey(w));
  return [1, i < 0 ? WINDOW_ORDER.length : i];
}

/** 短周期在前,长周期在后。同档位保持原顺序。 */
export function sortQuotaWindows(windows: QuotaWindow[]): QuotaWindow[] {
  return windows
    .map((w, i) => ({ w, i, rank: windowRank(w) }))
    .sort((a, b) => a.rank[0] - b.rank[0] || a.rank[1] - b.rank[1] || a.i - b.i)
    .map((x) => x.w);
}

function hasMeter(w: QuotaWindow): boolean {
  return typeof w.usedPercent === "number" && Number.isFinite(w.usedPercent);
}

function labelFor(id: string, windows: QuotaWindow[]): string {
  if (id === "__rest") return "";
  for (const w of windows) {
    const custom = w.groupLabel?.trim();
    if (custom) return custom;
  }
  return GROUP_LABELS[id] ?? id;
}

function groupRank(id: string): number {
  if (id === "__rest") return 10_000;
  const i = GROUP_ORDER.indexOf(id);
  return i < 0 ? 1_000 : i;
}

/**
 * 有池就分组;一个池都没有返回 null,调用方走原来的三列。
 * 有池但每段都缺百分比时返回空数组,调用方显示空状态,不补一条假进度。
 */
export function groupQuotaWindows(windows: QuotaWindow[]): QuotaGroup[] | null {
  if (!windows.some((w) => quotaGroupId(w))) return null;

  const buckets = new Map<string, { windows: QuotaWindow[]; seen: number }>();
  let seen = 0;
  for (const w of windows) {
    if (isExpiryWindow(w) || !hasMeter(w)) continue;
    const id = quotaGroupId(w) ?? "__rest";
    const bucket = buckets.get(id);
    if (bucket) bucket.windows.push(w);
    else buckets.set(id, { windows: [w], seen: seen++ });
  }

  return [...buckets.entries()]
    .map(([id, bucket]) => ({
      id,
      label: labelFor(id, bucket.windows),
      windows: sortQuotaWindows(bucket.windows),
      seen: bucket.seen,
      rank: groupRank(id),
    }))
    .sort((a, b) => a.rank - b.rank || a.seen - b.seen)
    .map(({ id, label, windows }) => ({ id, label, windows }));
}

export function quotaMeterLayout(windows: QuotaWindow[]): QuotaMeterLayout {
  const groups = groupQuotaWindows(windows);
  if (groups == null) return { kind: "flat" };
  if (groups.length === 0) return { kind: "empty" };
  return { kind: "groups", groups };
}

/** 一池一行最多两条。落单的那条(常见是只有每周的 Argon)横跨两列。 */
export function pairQuotaRows<T>(items: readonly T[]): T[][] {
  const rows: T[][] = [];
  for (let i = 0; i < items.length; i += 2) rows.push(items.slice(i, i + 2));
  return rows;
}
