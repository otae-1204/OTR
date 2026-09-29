/**
 * 主题颜色值的解析与归一化。
 *
 * 只接受有限的几种写法(见 `parseColor`),解析结果是纯数字,
 * 再由我们自己格式化成 HSL 三元组 / 十六进制 —— 主题文件里的字符串
 * **永远不会原样进入 CSS**,所以 `url()`、`expression()`、分号、大括号
 * 之类的东西根本没有落地的机会。
 */

export interface Rgba {
  r: number;
  g: number;
  b: number;
  /** 0–1 */
  a: number;
}

const HEX_RE = /^#([0-9a-f]{3}|[0-9a-f]{4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;
const RGB_RE =
  /^rgba?\(\s*(-?[\d.]+%?)\s*[,\s]\s*(-?[\d.]+%?)\s*[,\s]\s*(-?[\d.]+%?)\s*(?:[,/]\s*(-?[\d.]+%?)\s*)?\)$/i;
const HSL_RE =
  /^hsla?\(\s*(-?[\d.]+)(deg)?\s*[,\s]\s*(-?[\d.]+)%\s*[,\s]\s*(-?[\d.]+)%\s*(?:[,/]\s*(-?[\d.]+%?)\s*)?\)$/i;
/** 裸 HSL 三元组:与 index.css 里 `--background: 240 5% 12%` 同款,方便直接抄 */
const TRIPLET_RE = /^(-?[\d.]+)\s+(-?[\d.]+)%\s+(-?[\d.]+)%$/;

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}

function num(s: string): number | null {
  const v = Number(s);
  return Number.isFinite(v) ? v : null;
}

/** "255" → 255;"50%" → 127.5;超范围钳到 0–255 */
function channel(s: string): number | null {
  if (s.endsWith("%")) {
    const v = num(s.slice(0, -1));
    return v == null ? null : clamp((v / 100) * 255, 0, 255);
  }
  const v = num(s);
  return v == null ? null : clamp(v, 0, 255);
}

function alpha(s: string | undefined): number | null {
  if (s == null) return 1;
  if (s.endsWith("%")) {
    const v = num(s.slice(0, -1));
    return v == null ? null : clamp(v / 100, 0, 1);
  }
  const v = num(s);
  return v == null ? null : clamp(v, 0, 1);
}

export function hslToRgb(h: number, s: number, l: number): [number, number, number] {
  const hh = (((h % 360) + 360) % 360) / 360;
  const ss = clamp(s, 0, 100) / 100;
  const ll = clamp(l, 0, 100) / 100;
  if (ss === 0) {
    const v = ll * 255;
    return [v, v, v];
  }
  const q = ll < 0.5 ? ll * (1 + ss) : ll + ss - ll * ss;
  const p = 2 * ll - q;
  const f = (t: number) => {
    let tt = t;
    if (tt < 0) tt += 1;
    if (tt > 1) tt -= 1;
    if (tt < 1 / 6) return p + (q - p) * 6 * tt;
    if (tt < 1 / 2) return q;
    if (tt < 2 / 3) return p + (q - p) * (2 / 3 - tt) * 6;
    return p;
  };
  return [f(hh + 1 / 3) * 255, f(hh) * 255, f(hh - 1 / 3) * 255];
}

export function rgbToHsl(r: number, g: number, b: number): [number, number, number] {
  const rr = r / 255;
  const gg = g / 255;
  const bb = b / 255;
  const max = Math.max(rr, gg, bb);
  const min = Math.min(rr, gg, bb);
  const l = (max + min) / 2;
  if (max === min) return [0, 0, l * 100];
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h: number;
  if (max === rr) h = (gg - bb) / d + (gg < bb ? 6 : 0);
  else if (max === gg) h = (bb - rr) / d + 2;
  else h = (rr - gg) / d + 4;
  return [h * 60, s * 100, l * 100];
}

/**
 * 解析主题里的颜色字符串。支持:
 * - `#rgb` / `#rgba` / `#rrggbb` / `#rrggbbaa`
 * - `rgb(r, g, b)` / `rgb(r g b)` / `rgba(r, g, b, a)` / `rgb(r g b / a)`
 * - `hsl(h, s%, l%)` / `hsl(h s% l%)` / `hsla(...)` / `hsl(h s% l% / a)`
 * - 裸 HSL 三元组 `"240 5% 12%"`
 *
 * 不支持颜色关键字(`red`、`transparent`)、`color()`、`lab()` 等。
 * 解析失败返回 null。
 */
export function parseColor(input: unknown): Rgba | null {
  if (typeof input !== "string") return null;
  const s = input.trim();
  if (s.length === 0 || s.length > 64) return null;

  let m = HEX_RE.exec(s);
  if (m) {
    const hex = m[1];
    const expand = (i: number) =>
      hex.length <= 4 ? parseInt(hex[i] + hex[i], 16) : parseInt(hex.slice(i * 2, i * 2 + 2), 16);
    const hasAlpha = hex.length === 4 || hex.length === 8;
    return {
      r: expand(0),
      g: expand(1),
      b: expand(2),
      a: hasAlpha ? expand(3) / 255 : 1,
    };
  }

  m = RGB_RE.exec(s);
  if (m) {
    const r = channel(m[1]);
    const g = channel(m[2]);
    const b = channel(m[3]);
    const a = alpha(m[4]);
    if (r == null || g == null || b == null || a == null) return null;
    return { r, g, b, a };
  }

  m = HSL_RE.exec(s);
  if (m) {
    const h = num(m[1]);
    const sat = num(m[3]);
    const l = num(m[4]);
    const a = alpha(m[5]);
    if (h == null || sat == null || l == null || a == null) return null;
    const [r, g, b] = hslToRgb(h, sat, l);
    return { r, g, b, a };
  }

  m = TRIPLET_RE.exec(s);
  if (m) {
    const h = num(m[1]);
    const sat = num(m[2]);
    const l = num(m[3]);
    if (h == null || sat == null || l == null) return null;
    const [r, g, b] = hslToRgb(h, sat, l);
    return { r, g, b, a: 1 };
  }

  return null;
}

function round1(v: number): string {
  const r = Math.round(v * 10) / 10;
  return Number.isInteger(r) ? String(r) : r.toFixed(1);
}

/** → `"240 5% 12%"`(Tailwind `hsl(var(--x))` 需要的形式;忽略透明度) */
export function toHslTriplet(c: Rgba): string {
  const [h, s, l] = rgbToHsl(c.r, c.g, c.b);
  return `${round1(h)} ${round1(s)}% ${round1(l)}%`;
}

/** → `"#rrggbb"`(图表等需要具体颜色串的地方;忽略透明度) */
export function toHex(c: Rgba): string {
  const p = (v: number) => Math.round(clamp(v, 0, 255)).toString(16).padStart(2, "0");
  return `#${p(c.r)}${p(c.g)}${p(c.b)}`;
}

/** 是否完全不透明(语义色不允许带透明度:界面会在其上再叠加自己的透明度) */
export function isOpaque(c: Rgba): boolean {
  return c.a >= 0.999;
}
