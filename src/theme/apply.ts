/**
 * 把 ResolvedTheme 落到 DOM:`<html>` 的 `.dark` class、color-scheme、CSS 变量、受限自定义 CSS。
 *
 * 变量写在 `document.documentElement.style` 上(内联样式),优先级高于 index.css 里
 * `:root` / `.dark` 的兜底值;JS 没跑起来之前页面用的就是那份兜底(= 默认主题)。
 * 受限自定义 CSS(`css` 字段)序列化后放进**唯一一个**受控的 `<style id="otr-theme-css">`,
 * 每次应用主题整体替换其内容,主题没有 css 时移除该元素;它始终被挪到 <head> 末尾,
 * 保证在同权重下覆盖 Tailwind 的工具类。
 * 同时把上次应用的变量与 css 缓存进 localStorage,下次启动在 React 挂载前先同步刷一遍,
 * 避免自定义主题的用户每次启动都先闪一下默认配色。
 */

import { sanitizeCss, serializeThemeCss } from "./css";
import type { ResolvedTheme, ThemeCss, ThemeMode } from "./types";

/** 深浅模式的本机记忆键(历史键名,保持兼容) */
export const MODE_STORAGE_KEY = "token-show-theme";
/** 上次应用的主题变量缓存 */
export const THEME_CACHE_KEY = "otr-theme-cache";
/** 受限自定义 CSS 的唯一注入点 */
export const THEME_STYLE_ID = "otr-theme-css";

interface ThemeCache {
  id: string;
  mode: ThemeMode;
  vars: Record<string, string>;
  /** 受限自定义 CSS 的表(不是文本);读回时整张重新校验 */
  css?: ThemeCss;
}

/** 已写到 <html> 上的变量名;切主题时把不再需要的删掉 */
const applied = new Set<string>();

export function normalizeMode(v: unknown): ThemeMode | null {
  return v === "dark" || v === "light" ? v : null;
}

/** 本机记忆的模式;没有记忆时默认暗色(与 index.html 的初始 class 一致) */
export function readStoredMode(): ThemeMode {
  try {
    return normalizeMode(localStorage.getItem(MODE_STORAGE_KEY)) ?? "dark";
  } catch {
    return "dark";
  }
}

export function storeMode(mode: ThemeMode): void {
  try {
    localStorage.setItem(MODE_STORAGE_KEY, mode);
  } catch {
    // 写不进去只影响下次启动的首帧
  }
}

function readCache(): ThemeCache | null {
  try {
    const raw = localStorage.getItem(THEME_CACHE_KEY);
    if (!raw) return null;
    const c = JSON.parse(raw) as Partial<ThemeCache>;
    if (
      typeof c.id !== "string" ||
      !normalizeMode(c.mode) ||
      typeof c.vars !== "object" ||
      c.vars === null
    ) {
      return null;
    }
    const vars: Record<string, string> = {};
    for (const [k, v] of Object.entries(c.vars)) {
      if (/^--[a-z0-9-]+$/.test(k) && typeof v === "string") vars[k] = v;
    }
    // 缓存里的 css 表不信任,整张过一遍与清单相同的校验
    const css = c.css === undefined ? undefined : sanitizeCss(c.css);
    return { id: c.id, mode: c.mode as ThemeMode, vars, css };
  } catch {
    return null;
  }
}

function writeCache(theme: ResolvedTheme): void {
  try {
    const c: ThemeCache = { id: theme.id, mode: theme.mode, vars: theme.cssVars };
    if (Object.keys(theme.css).length > 0) c.css = theme.css;
    localStorage.setItem(THEME_CACHE_KEY, JSON.stringify(c));
  } catch {
    // 忽略
  }
}

/**
 * 把受限自定义 CSS 写进唯一的受控 <style>。空文本 → 移除元素。
 * 元素每次都 appendChild 到 <head> 末尾:已存在时相当于挪到最后,
 * 这样即使开发模式下 Vite 后来又注入了样式,主题规则仍在最后。
 */
function applyCss(css: ThemeCss | undefined): void {
  const text = serializeThemeCss(css);
  let el = document.getElementById(THEME_STYLE_ID);
  if (!text) {
    el?.remove();
    return;
  }
  if (!el || el.tagName !== "STYLE") {
    el?.remove();
    el = document.createElement("style");
    el.id = THEME_STYLE_ID;
  }
  if (el.textContent !== text) el.textContent = text;
  document.head.appendChild(el);
}

function applyMode(mode: ThemeMode): void {
  const root = document.documentElement;
  root.classList.toggle("dark", mode === "dark");
  root.style.colorScheme = mode;
}

function applyVars(vars: Record<string, string>): void {
  const style = document.documentElement.style;
  for (const name of applied) {
    if (!(name in vars)) {
      style.removeProperty(name);
      applied.delete(name);
    }
  }
  for (const [name, value] of Object.entries(vars)) {
    if (value === "") {
      style.removeProperty(name);
      applied.delete(name);
      continue;
    }
    style.setProperty(name, value);
    applied.add(name);
  }
}

/** 应用一份解析好的主题,并记入缓存 */
export function applyResolvedTheme(theme: ResolvedTheme): void {
  applyMode(theme.mode);
  applyVars(theme.cssVars);
  applyCss(theme.css);
  document.documentElement.dataset.theme = theme.id;
  storeMode(theme.mode);
  writeCache(theme);
}

/**
 * 启动首帧(React 挂载前)调用:同步应用本机记忆的模式与上次的变量缓存。
 * 缓存的模式与记忆的模式不一致时只切模式、不用缓存(颜色是按模式存的)。
 */
export function bootTheme(): void {
  const mode = readStoredMode();
  applyMode(mode);
  const cache = readCache();
  if (cache && cache.mode === mode) {
    applyVars(cache.vars);
    applyCss(cache.css);
    document.documentElement.dataset.theme = cache.id;
  }
}
