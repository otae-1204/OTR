/**
 * 主题接口(Theme API)的类型与常量。
 *
 * 一个主题 = 一份 JSON 清单(manifest):元数据 + 设计 token。
 * token 分「公共 token」(`tokens`,与深浅模式无关)和「按模式 token」
 * (`modes.dark` / `modes.light`),两者结构相同;按模式的覆盖公共的,
 * 公共的覆盖内置默认主题。缺失的 token 一律回退到内置默认主题同模式的值。
 *
 * 详细说明见 docs/theme_interface.md。
 */

/** 当前应用支持的主题格式版本。清单里的 `apiVersion` 必须等于它(或更旧且可迁移)。 */
export const THEME_API_VERSION = 1;

/** 内置默认主题的 id;所有缺失 token 的最终回退来源 */
export const DEFAULT_THEME_ID = "otr";

/** 单个主题文件的大小上限(字节),与 Rust 侧 `themes::MAX_THEME_FILE_BYTES` 一致 */
export const MAX_THEME_FILE_BYTES = 256 * 1024;

export type ThemeMode = "dark" | "light";

export const THEME_MODES: readonly ThemeMode[] = ["dark", "light"];

/** 语义色 token(HSL 三元组,供 Tailwind `hsl(var(--x))` 使用;不允许透明度) */
export const COLOR_TOKENS = [
  "background",
  "foreground",
  "card",
  "cardForeground",
  "popover",
  "popoverForeground",
  "primary",
  "primaryForeground",
  "secondary",
  "secondaryForeground",
  "muted",
  "mutedForeground",
  "accent",
  "accentForeground",
  "destructive",
  "destructiveForeground",
  "border",
  "input",
  "ring",
  /** hover 高亮叠加的基色(界面按 5% 透明度叠加) */
  "overlay",
  /** 状态色:填充(进度条、圆点)用 */
  "success",
  "warning",
  "danger",
  "info",
  /** 「有新版本」之类需要注意但非警告的标记 */
  "notice",
  /** 状态色:放在浅色底上的文字用(默认亮色更深、暗色更浅) */
  "successText",
  "warningText",
  "dangerText",
] as const;
export type ColorToken = (typeof COLOR_TOKENS)[number];

/** 统计卡各项指标的强调色(输入/输出/缓存读/缓存写/请求/成本) */
export const STAT_TOKENS = [
  "input",
  "output",
  "cacheRead",
  "cacheWrite",
  "calls",
  "cost",
] as const;
export type StatToken = (typeof STAT_TOKENS)[number];

export const FONT_TOKENS = ["sans", "mono"] as const;
export type FontToken = (typeof FONT_TOKENS)[number];

export const RADIUS_TOKENS = ["md", "lg", "xl"] as const;
export type RadiusToken = (typeof RADIUS_TOKENS)[number];

export const SHADOW_TOKENS = ["sm", "base", "md", "lg"] as const;
export type ShadowToken = (typeof SHADOW_TOKENS)[number];

/** 图表调色板长度上限 */
export const MAX_PALETTE_LENGTH = 16;

/**
 * 受限自定义 CSS:`{ "钩子[:状态]": { "属性": "值" } }`(校验后的值已归一化)。
 * 钩子目录、状态、属性白名单与值语法见 css.ts / 文档 §14。
 */
export type ThemeCss = Record<string, Record<string, string>>;

/**
 * 一组 token(公共或某个模式)。所有字段都可选:缺的回退。
 * 颜色值接受 `#rgb` / `#rrggbb` / `rgb()` / `hsl()` / 裸 HSL 三元组 `"240 5% 12%"`。
 */
export interface ThemeTokens {
  colors?: Partial<Record<ColorToken, string>>;
  stat?: Partial<Record<StatToken, string>>;
  chart?: {
    /** 模型占比等「按序取色」的调色板(1–16 个) */
    palette?: string[];
    /** 各 Agent 的品牌色:id → 颜色。可以只覆盖其中几个,也可以给自定义 Agent 配色 */
    agents?: Record<string, string>;
    /** 未指定品牌色的 Agent 按 id 哈希从这里取色(1–16 个) */
    agentFallback?: string[];
  };
  font?: Partial<Record<FontToken, string>>;
  radius?: Partial<Record<RadiusToken, string>>;
  shadow?: Partial<Record<ShadowToken, string>>;
  /** 受限自定义 CSS(apiVersion 1 内新增的可选字段;老应用忽略并告警) */
  css?: ThemeCss;
}

/** 校验通过后的主题清单(已剔除未知字段与非法 token) */
export interface ThemeManifest {
  apiVersion: number;
  id: string;
  name: string;
  version?: string;
  author?: string;
  description?: string;
  /** 只作文本展示,永不请求 */
  homepage?: string;
  tokens?: ThemeTokens;
  modes: Partial<Record<ThemeMode, ThemeTokens>>;
}

export interface ThemeDiagnostic {
  level: "error" | "warning";
  /** JSON 路径,如 "modes.dark.colors.primary" */
  path: string;
  message: string;
}

export interface ValidationResult {
  manifest: ThemeManifest | null;
  diagnostics: ThemeDiagnostic[];
}

/** 主题来源:内置(随应用打包)或用户目录里的文件 */
export type ThemeSource = "builtin" | "user";

/** 发现到的一个主题(可能校验失败,此时 manifest 为 null) */
export interface ThemeEntry {
  /** 校验通过时 = manifest.id;失败时用文件名兜底,仅供展示 */
  id: string;
  name: string;
  source: ThemeSource;
  /** 用户主题的文件路径;内置为 null */
  path: string | null;
  manifest: ThemeManifest | null;
  diagnostics: ThemeDiagnostic[];
  /** 该主题提供了哪些模式(校验失败时为空) */
  modes: ThemeMode[];
}

/** 完整解析后的、可以直接落到 DOM 上的主题 */
export interface ResolvedTheme {
  id: string;
  name: string;
  source: ThemeSource;
  mode: ThemeMode;
  /** CSS 自定义属性:`--background` → `"240 5% 12%"` 等 */
  cssVars: Record<string, string>;
  chart: {
    palette: string[];
    agents: Record<string, string>;
    agentFallback: string[];
  };
  /** 该模式下叠加后的受限自定义 CSS(键 → 属性 → 归一化值);没有时为空对象 */
  css: ThemeCss;
}

/** Rust `list_themes` 命令返回的一条记录 */
export interface ThemeFile {
  path: string;
  /** 文件名(去扩展名)或目录名,校验失败时用作展示名 */
  name: string;
  contents: string | null;
  error: string | null;
}
