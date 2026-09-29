/**
 * 主题清单校验:把「不可信的 JSON」变成「类型明确、值已归一化」的 ThemeManifest。
 *
 * 原则:
 * - 结构性错误(不是对象、缺 id、apiVersion 不支持、一个模式都没有)→ error,整份拒绝;
 * - 单个 token 非法 → warning,丢掉该 token(之后回退到默认值),其余照常;
 * - 未知字段 → warning,忽略(向前兼容:新版主题用了老版本不认识的 token 也不会整份失效)。
 * - 所有值都经过白名单语法校验并重新格式化,主题文件里的原始字符串不会直接进入 CSS。
 */

import {
  isOpaque,
  parseColor,
  toHex,
  toHslTriplet,
} from "./color";
import { validateCss } from "./css";
import {
  normalizeFontFamily,
  normalizeLength,
  normalizeShadow,
} from "./values";
import {
  COLOR_TOKENS,
  FONT_TOKENS,
  MAX_PALETTE_LENGTH,
  MAX_THEME_FILE_BYTES,
  RADIUS_TOKENS,
  SHADOW_TOKENS,
  STAT_TOKENS,
  THEME_API_VERSION,
  THEME_MODES,
  type ThemeDiagnostic,
  type ThemeManifest,
  type ThemeMode,
  type ThemeTokens,
  type ValidationResult,
} from "./types";

/** 主题 id / Agent id:小写字母数字开头,允许 `-` `_` `.`,最长 64 */
export const ID_RE = /^[a-z0-9][a-z0-9._-]{0,63}$/;

const MAX_NAME_LEN = 64;
const MAX_TEXT_LEN = 200;
const CONTROL_RE = /[\u0000-\u001f\u007f]/;

class Diag {
  readonly list: ThemeDiagnostic[] = [];
  error(path: string, message: string) {
    this.list.push({ level: "error", path, message });
  }
  warn(path: string, message: string) {
    this.list.push({ level: "warning", path, message });
  }
  get hasError(): boolean {
    return this.list.some((d) => d.level === "error");
  }
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function joinPath(base: string, key: string): string {
  return base ? `${base}.${key}` : key;
}

/** 把「未知键」统一报成 warning,方便作者发现拼写错误 */
function warnUnknownKeys(
  obj: Record<string, unknown>,
  known: readonly string[],
  path: string,
  d: Diag,
) {
  for (const k of Object.keys(obj)) {
    if (!known.includes(k)) {
      d.warn(joinPath(path, k), `未知字段,已忽略`);
    }
  }
}

function optionalText(
  obj: Record<string, unknown>,
  key: string,
  maxLen: number,
  d: Diag,
): string | undefined {
  const v = obj[key];
  if (v === undefined || v === null) return undefined;
  if (typeof v !== "string") {
    d.warn(key, "应为字符串,已忽略");
    return undefined;
  }
  const s = v.trim();
  if (!s) return undefined;
  if (s.length > maxLen || CONTROL_RE.test(s)) {
    d.warn(key, `超过 ${maxLen} 个字符或含控制字符,已忽略`);
    return undefined;
  }
  return s;
}

export { normalizeFontFamily, normalizeLength, normalizeShadow };

/** 语义色:必须不透明,归一化成 HSL 三元组 */
export function normalizeSemanticColor(v: unknown): string | null {
  const c = parseColor(v);
  if (!c || !isOpaque(c)) return null;
  return toHslTriplet(c);
}

/** 图表色:归一化成 #rrggbb(透明度忽略) */
export function normalizeChartColor(v: unknown): string | null {
  const c = parseColor(v);
  if (!c || !isOpaque(c)) return null;
  return toHex(c);
}

function validateColorGroup<K extends string>(
  raw: unknown,
  keys: readonly K[],
  path: string,
  normalize: (v: unknown) => string | null,
  what: string,
  d: Diag,
): Partial<Record<K, string>> | undefined {
  if (raw === undefined) return undefined;
  if (!isPlainObject(raw)) {
    d.warn(path, "应为对象,已忽略");
    return undefined;
  }
  warnUnknownKeys(raw, keys, path, d);
  const out: Partial<Record<K, string>> = {};
  for (const k of keys) {
    if (raw[k] === undefined) continue;
    const n = normalize(raw[k]);
    if (n == null) {
      d.warn(joinPath(path, k), `${what}格式无效,已回退默认值`);
      continue;
    }
    out[k] = n;
  }
  return out;
}

function validateColorList(
  raw: unknown,
  path: string,
  d: Diag,
): string[] | undefined {
  if (raw === undefined) return undefined;
  if (!Array.isArray(raw) || raw.length === 0 || raw.length > MAX_PALETTE_LENGTH) {
    d.warn(path, `应为 1–${MAX_PALETTE_LENGTH} 个颜色的数组,已回退默认值`);
    return undefined;
  }
  const out: string[] = [];
  raw.forEach((v, i) => {
    const n = normalizeChartColor(v);
    if (n == null) {
      d.warn(`${path}[${i}]`, "颜色格式无效,已跳过该项");
    } else {
      out.push(n);
    }
  });
  if (out.length === 0) {
    d.warn(path, "没有一个有效颜色,已回退默认值");
    return undefined;
  }
  return out;
}

function validateChart(raw: unknown, path: string, d: Diag): ThemeTokens["chart"] {
  if (raw === undefined) return undefined;
  if (!isPlainObject(raw)) {
    d.warn(path, "应为对象,已忽略");
    return undefined;
  }
  warnUnknownKeys(raw, ["palette", "agents", "agentFallback"], path, d);
  const out: NonNullable<ThemeTokens["chart"]> = {};
  const palette = validateColorList(raw.palette, joinPath(path, "palette"), d);
  if (palette) out.palette = palette;
  const fallback = validateColorList(raw.agentFallback, joinPath(path, "agentFallback"), d);
  if (fallback) out.agentFallback = fallback;
  if (raw.agents !== undefined) {
    const p = joinPath(path, "agents");
    if (!isPlainObject(raw.agents)) {
      d.warn(p, "应为 { agentId: 颜色 } 对象,已忽略");
    } else {
      const agents: Record<string, string> = {};
      for (const [id, v] of Object.entries(raw.agents)) {
        if (!ID_RE.test(id)) {
          d.warn(joinPath(p, id), "Agent id 不合法(小写字母数字、- _ .,最长 64),已忽略");
          continue;
        }
        const n = normalizeChartColor(v);
        if (n == null) {
          d.warn(joinPath(p, id), "颜色格式无效,已回退默认值");
          continue;
        }
        agents[id] = n;
      }
      if (Object.keys(agents).length > 0) out.agents = agents;
    }
  }
  return out;
}

const TOKEN_GROUPS = ["colors", "stat", "chart", "font", "radius", "shadow", "css"] as const;

/** 校验一组 token(公共或某模式),非法 token 丢弃并记 warning */
export function validateTokens(raw: unknown, path: string, d: Diag): ThemeTokens | undefined {
  if (raw === undefined) return undefined;
  if (!isPlainObject(raw)) {
    d.warn(path, "应为对象,已忽略");
    return undefined;
  }
  warnUnknownKeys(raw, TOKEN_GROUPS, path, d);
  const out: ThemeTokens = {};
  const colors = validateColorGroup(
    raw.colors, COLOR_TOKENS, joinPath(path, "colors"),
    normalizeSemanticColor, "颜色(须不透明)", d,
  );
  if (colors) out.colors = colors;
  const stat = validateColorGroup(
    raw.stat, STAT_TOKENS, joinPath(path, "stat"),
    normalizeSemanticColor, "颜色(须不透明)", d,
  );
  if (stat) out.stat = stat;
  const chart = validateChart(raw.chart, joinPath(path, "chart"), d);
  if (chart) out.chart = chart;
  const font = validateColorGroup(
    raw.font, FONT_TOKENS, joinPath(path, "font"),
    normalizeFontFamily, "字体族", d,
  );
  if (font) out.font = font;
  const radius = validateColorGroup(
    raw.radius, RADIUS_TOKENS, joinPath(path, "radius"),
    normalizeLength, "长度(0 或 px/rem/em)", d,
  );
  if (radius) out.radius = radius;
  const shadow = validateColorGroup(
    raw.shadow, SHADOW_TOKENS, joinPath(path, "shadow"),
    normalizeShadow, "box-shadow", d,
  );
  if (shadow) out.shadow = shadow;
  const css = validateCss(raw.css, joinPath(path, "css"), (p, m) => d.warn(p, m));
  if (css && Object.keys(css).length > 0) out.css = css;
  return out;
}

const TOP_LEVEL_KEYS = [
  "$schema",
  "apiVersion",
  "id",
  "name",
  "version",
  "author",
  "description",
  "homepage",
  "tokens",
  "modes",
] as const;

/**
 * 校验一份已经 JSON.parse 过的清单。
 * `reservedIds`:不允许用户主题占用的 id(内置主题的 id)。
 */
export function validateManifest(
  raw: unknown,
  opts: { reservedIds?: readonly string[] } = {},
): ValidationResult {
  const d = new Diag();
  if (!isPlainObject(raw)) {
    d.error("", "主题文件顶层必须是 JSON 对象");
    return { manifest: null, diagnostics: d.list };
  }
  warnUnknownKeys(raw, TOP_LEVEL_KEYS, "", d);

  // apiVersion:必须是整数;比当前新 → 拒绝(语义未知);比当前旧 → 目前没有旧版本,同样拒绝
  const apiVersion = raw.apiVersion;
  if (typeof apiVersion !== "number" || !Number.isInteger(apiVersion)) {
    d.error("apiVersion", `缺少或不是整数(当前支持 ${THEME_API_VERSION})`);
  } else if (apiVersion > THEME_API_VERSION) {
    d.error(
      "apiVersion",
      `主题需要更新的 OTR(主题格式 v${apiVersion},本版本支持 v${THEME_API_VERSION})`,
    );
  } else if (apiVersion < 1) {
    d.error("apiVersion", `不支持的主题格式版本 ${apiVersion}`);
  }

  const id = typeof raw.id === "string" ? raw.id.trim() : "";
  if (!ID_RE.test(id)) {
    d.error("id", "缺少或不合法:小写字母/数字开头,只允许 a-z 0-9 - _ .,最长 64");
  } else if (opts.reservedIds?.includes(id)) {
    d.error("id", `「${id}」是内置主题的 id,用户主题不能使用`);
  }

  const name = optionalText(raw, "name", MAX_NAME_LEN, d);
  if (!name) d.error("name", `缺少或不合法(非空字符串,最长 ${MAX_NAME_LEN})`);

  const version = optionalText(raw, "version", 32, d);
  const author = optionalText(raw, "author", MAX_TEXT_LEN, d);
  const description = optionalText(raw, "description", MAX_TEXT_LEN, d);
  let homepage = optionalText(raw, "homepage", MAX_TEXT_LEN, d);
  if (homepage && !/^https?:\/\/[^\s]+$/i.test(homepage)) {
    d.warn("homepage", "应为 http(s) 链接,已忽略(仅作文本展示,不会访问)");
    homepage = undefined;
  }

  const tokens = validateTokens(raw.tokens, "tokens", d);

  const modes: Partial<Record<ThemeMode, ThemeTokens>> = {};
  if (!isPlainObject(raw.modes)) {
    d.error("modes", "缺少 modes:至少要有 dark 或 light 其中之一");
  } else {
    warnUnknownKeys(raw.modes, THEME_MODES, "modes", d);
    for (const m of THEME_MODES) {
      if (raw.modes[m] === undefined) continue;
      const t = validateTokens(raw.modes[m], `modes.${m}`, d);
      // 允许 `"dark": {}`:表示「提供该模式,但全部用默认值」
      modes[m] = t ?? {};
    }
    if (Object.keys(modes).length === 0) {
      d.error("modes", "至少要有 dark 或 light 其中之一");
    }
  }

  if (d.hasError) return { manifest: null, diagnostics: d.list };
  return {
    manifest: {
      apiVersion: apiVersion as number,
      id,
      name: name as string,
      version,
      author,
      description,
      homepage,
      tokens,
      modes,
    },
    diagnostics: d.list,
  };
}

/** 从文件文本到清单:先看大小,再 JSON.parse,再校验 */
export function parseThemeFile(
  text: string,
  opts: { reservedIds?: readonly string[] } = {},
): ValidationResult {
  if (text.length > MAX_THEME_FILE_BYTES) {
    return {
      manifest: null,
      diagnostics: [
        { level: "error", path: "", message: `文件超过 ${MAX_THEME_FILE_BYTES / 1024} KiB 上限` },
      ],
    };
  }
  let raw: unknown;
  try {
    // Windows 上的记事本 / PowerShell 5.1 常写出带 BOM 的 UTF-8,JSON.parse 不认,先去掉
    raw = JSON.parse(text.charCodeAt(0) === 0xfeff ? text.slice(1) : text);
  } catch (err) {
    return {
      manifest: null,
      diagnostics: [
        { level: "error", path: "", message: `不是合法 JSON:${(err as Error).message}` },
      ],
    };
  }
  return validateManifest(raw, opts);
}
