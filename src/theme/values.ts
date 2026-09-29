/**
 * token 值的白名单语法与归一化(字体族 / 长度 / 阴影)。
 *
 * 从 validate.ts 拆出来,供 token 校验(validate.ts)与受限 css(css.ts)共用。
 * 所有函数的约定相同:输入不可信字符串,输出**重新格式化**后的字符串,不合法返回 null;
 * 主题文件里的原始字符串永远不会原样进入 CSS。
 */

import { parseColor } from "./color";

const MAX_TEXT_LEN = 200;

/** 字体族:字母/数字/空格/引号/逗号/连字符/下划线/点(含 CJK 等 Unicode 字母)。
 *  括号、分号、斜杠、反斜杠、尖括号、@ 都不在白名单里,`url(`、`@import` 无从构造。 */
const FONT_FAMILY_RE = /^[\p{L}\p{N} _"'.,\-]+$/u;

/** CSS 长度:0 或 带 px/rem/em 单位的非负数 */
export const LENGTH_RE = /^(0|(\d+(\.\d+)?|\.\d+)(px|rem|em))$/;
/** 阴影里的偏移量允许负数 */
export const SIGNED_LENGTH_RE = /^(0|-?(\d+(\.\d+)?|\.\d+)(px|rem|em))$/;

export const MAX_LENGTH_PX = 128;
export const MAX_LENGTH_REM = 8;

export function normalizeFontFamily(v: unknown): string | null {
  if (typeof v !== "string") return null;
  const s = v.trim();
  if (!s || s.length > MAX_TEXT_LEN || !FONT_FAMILY_RE.test(s)) return null;
  const items = s.split(",").map((x) => x.trim());
  if (items.some((x) => !x)) return null;
  // 引号必须成对且只包一整个族名
  for (const item of items) {
    const q = item[0] === '"' || item[0] === "'" ? item[0] : null;
    if (q) {
      if (item.length < 3 || item[item.length - 1] !== q) return null;
      if (item.slice(1, -1).includes(q)) return null;
    } else if (item.includes('"') || item.includes("'")) {
      return null;
    }
  }
  return items.join(", ");
}

export function lengthInRange(s: string, re: RegExp): boolean {
  if (!re.test(s)) return false;
  if (s === "0") return true;
  const v = Math.abs(parseFloat(s));
  if (s.endsWith("px")) return v <= MAX_LENGTH_PX;
  return v <= MAX_LENGTH_REM;
}

export function normalizeLength(v: unknown): string | null {
  if (typeof v !== "string") return null;
  const s = v.trim();
  return lengthInRange(s, LENGTH_RE) ? s : null;
}

/** 阴影里的颜色统一写成 `rgb(r g b / a)` */
export function formatShadowColor(c: { r: number; g: number; b: number; a: number }): string {
  return `rgb(${Math.round(c.r)} ${Math.round(c.g)} ${Math.round(c.b)} / ${Math.round(c.a * 1000) / 1000})`;
}

export interface ShadowOptions {
  /** 是否允许 `inset`(box-shadow 允许,text-shadow 不允许) */
  inset: boolean;
  /** 每层长度个数范围:box-shadow 2–4,text-shadow 2–3 */
  minLengths: number;
  maxLengths: number;
}

const BOX_SHADOW: ShadowOptions = { inset: true, minLengths: 2, maxLengths: 4 };
const TEXT_SHADOW: ShadowOptions = { inset: false, minLengths: 2, maxLengths: 3 };

/**
 * 阴影:`none`,或逗号分隔的若干层;每层 = [inset] N 个长度 [颜色]。
 * 颜色允许透明度(阴影天然需要)。其它任何东西(url、var、关键字)都不接受。
 */
export function normalizeShadowWith(v: unknown, opts: ShadowOptions): string | null {
  if (typeof v !== "string") return null;
  const s = v.trim();
  if (!s || s.length > MAX_TEXT_LEN) return null;
  if (s.toLowerCase() === "none") return "none";
  // 先把带空格/逗号的颜色函数换成占位符,再按逗号分层
  const colors: string[] = [];
  const masked = s.replace(/(rgba?|hsla?)\([^()]*\)/gi, (m) => {
    colors.push(m);
    return `\u0000${colors.length - 1}\u0000`;
  });
  if (/[()]/.test(masked)) return null;
  const layers = masked.split(",").map((x) => x.trim());
  const out: string[] = [];
  for (const layer of layers) {
    if (!layer) return null;
    const parts = layer.split(/\s+/);
    const built: string[] = [];
    let lengths = 0;
    let color: string | null = null;
    for (const raw of parts) {
      const part = raw.replace(/\u0000(\d+)\u0000/g, (_, i) => colors[Number(i)]);
      if (part.toLowerCase() === "inset") {
        if (!opts.inset || built.length > 0) return null;
        built.push("inset");
        continue;
      }
      if (lengthInRange(part, SIGNED_LENGTH_RE)) {
        if (color) return null;
        lengths++;
        built.push(part);
        continue;
      }
      const c = parseColor(part);
      if (c && !color) {
        color = formatShadowColor(c);
        built.push(color);
        continue;
      }
      return null;
    }
    if (lengths < opts.minLengths || lengths > opts.maxLengths) return null;
    out.push(built.join(" "));
  }
  return out.join(", ");
}

/** box-shadow:`none`,或逗号分隔的若干层;每层 = [inset] 2–4 个长度 [颜色] */
export function normalizeShadow(v: unknown): string | null {
  return normalizeShadowWith(v, BOX_SHADOW);
}

/** text-shadow:每层 2–3 个长度 [颜色],不允许 inset */
export function normalizeTextShadow(v: unknown): string | null {
  return normalizeShadowWith(v, TEXT_SHADOW);
}
