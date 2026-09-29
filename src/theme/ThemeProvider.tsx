import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api/bindings";
import {
  applyResolvedTheme,
  normalizeMode,
  readPreferredMode,
  readStoredMode,
  resetThemeDom,
  storePreferredMode,
} from "./apply";
import { OTR_THEME } from "./builtin";
import { loadThemeListing, builtinEntries } from "./registry";
import { pickAgentColor, resolveTheme } from "./resolve";
import {
  DEFAULT_THEME_ID,
  type ResolvedTheme,
  type ThemeEntry,
  type ThemeMode,
} from "./types";

/** 托盘「恢复默认主题」:Rust 已把 settings.json 的 themeId 改回 `otr`,前端收到后重新应用 */
export const THEME_RESET_EVENT = "theme://reset";

export interface ApplyOutcome {
  /** 实际生效的主题 id(选中的主题不可用时是默认主题) */
  id: string;
  /** 实际生效的模式(主题不支持偏好模式时会临时用它支持的模式) */
  mode: ThemeMode;
  /** 用户偏好的模式(单模式主题不会改变它;调用方把它存进 settings.preferredMode) */
  preferredMode: ThemeMode;
  /** 没能按所选应用时的原因;正常时为 null */
  fallbackReason: string | null;
}

export interface ThemeContextValue {
  /** 当前已应用的主题 */
  theme: ResolvedTheme;
  /** 生效模式(= theme.mode) */
  mode: ThemeMode;
  /** 偏好模式:设置页深浅按钮改的是它;选中单模式主题时它保持不变 */
  preferredMode: ThemeMode;
  /** 设置里记的主题 id(可能已不存在,见 fallbackReason) */
  selectedId: string;
  fallbackReason: string | null;
  entries: ThemeEntry[];
  themesDir: string | null;
  listingError: string | null;
  loading: boolean;
  /** 应用某个主题,模式按偏好(不负责持久化;调用方自己把 id / 模式存进设置) */
  selectTheme: (id: string) => ApplyOutcome;
  /** 修改偏好模式并按它重新应用(不负责持久化) */
  setMode: (mode: ThemeMode) => ApplyOutcome;
  /** 重新扫描主题目录,并按当前选择重新应用 */
  reload: () => Promise<void>;
  /** 托盘「恢复默认主题」已处理的次数;设置页据此刷新自己的设置快照 */
  resetSeq: number;
  agentColor: (id: string) => string;
  chartPalette: string[];
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

function initialTheme(): ResolvedTheme {
  return resolveTheme(OTR_THEME, readStoredMode(), "builtin");
}

const MODE_LABEL: Record<ThemeMode, string> = { dark: "暗色", light: "亮色" };

/**
 * 决定「选了 id、偏好 mode」时实际应用什么。
 * - id 找不到 / 校验失败 → 默认主题 + 原因
 * - 主题不支持 mode → 临时用它支持的第一个模式 + 原因(偏好不变,返回值里原样带回)
 */
export function decide(
  entries: ThemeEntry[],
  id: string,
  mode: ThemeMode,
): { theme: ResolvedTheme; outcome: ApplyOutcome } {
  const entry = entries.find((e) => e.id === id && e.manifest);
  if (!entry || !entry.manifest) {
    const fallback = entries.find((e) => e.id === DEFAULT_THEME_ID && e.manifest);
    const manifest = fallback?.manifest ?? OTR_THEME;
    const theme = resolveTheme(manifest, mode, "builtin");
    const found = entries.find((e) => e.id === id);
    return {
      theme,
      outcome: {
        id: manifest.id,
        mode,
        preferredMode: mode,
        fallbackReason: found
          ? `主题「${found.name}」校验未通过,已回退默认主题`
          : `找不到主题「${id}」,已回退默认主题`,
      },
    };
  }
  let useMode = mode;
  let reason: string | null = null;
  if (!entry.modes.includes(mode)) {
    useMode = entry.modes[0] ?? mode;
    reason = `主题「${entry.name}」没有${MODE_LABEL[mode]}模式,暂用${MODE_LABEL[useMode]}`;
  }
  return {
    theme: resolveTheme(entry.manifest, useMode, entry.source),
    outcome: { id: entry.id, mode: useMode, preferredMode: mode, fallbackReason: reason },
  };
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setTheme] = useState<ResolvedTheme>(initialTheme);
  const [entries, setEntries] = useState<ThemeEntry[]>(builtinEntries);
  const [themesDir, setThemesDir] = useState<string | null>(null);
  const [listingError, setListingError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string>(DEFAULT_THEME_ID);
  const [preferredMode, setPreferredMode] = useState<ThemeMode>(readPreferredMode);
  const [fallbackReason, setFallbackReason] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [resetSeq, setResetSeq] = useState(0);
  const entriesRef = useRef(entries);
  entriesRef.current = entries;

  /** 按「主题 id + 偏好模式」应用;生效模式由 decide() 决定,偏好原样记下 */
  const applyWith = useCallback(
    (list: ThemeEntry[], id: string, preferred: ThemeMode): ApplyOutcome => {
      const { theme: next, outcome } = decide(list, id, preferred);
      try {
        applyResolvedTheme(next);
      } catch (err) {
        console.error("[theme] 应用主题失败", err);
      }
      storePreferredMode(preferred);
      setTheme(next);
      setSelectedId(id);
      setPreferredMode(preferred);
      setFallbackReason(outcome.fallbackReason);
      return outcome;
    },
    [],
  );

  const bootstrap = useCallback(
    async (idOverride?: string, modeOverride?: ThemeMode) => {
      setLoading(true);
      const [settings, listing] = await Promise.all([
        api.getSettings().catch((err) => {
          console.error("[theme] getSettings 失败", err);
          return null;
        }),
        loadThemeListing(),
      ]);
      setEntries(listing.entries);
      setThemesDir(listing.themesDir);
      setListingError(listing.error);
      const id = idOverride ?? settings?.themeId ?? DEFAULT_THEME_ID;
      // 设置文件是持久化的真相;localStorage 只是兜底。旧设置文件没有 preferredMode 时
      // 用 theme(上次生效的模式)推导 —— Rust 读设置时已经推导过一次,这里再兜一层
      const preferred =
        modeOverride ??
        normalizeMode(settings?.preferredMode) ??
        normalizeMode(settings?.theme) ??
        readPreferredMode();
      applyWith(listing.entries, id, preferred);
      setLoading(false);
    },
    [applyWith],
  );

  useEffect(() => {
    void bootstrap();
  }, [bootstrap]);

  // 托盘「恢复默认主题」:先把 DOM 恢复成兜底外观(当前主题再坏也立刻可见),再按设置文件重新应用
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen(THEME_RESET_EVENT, () => {
      resetThemeDom();
      void bootstrap().finally(() => setResetSeq((n) => n + 1));
    })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch((err) => console.error("[theme] 订阅 theme://reset 失败", err));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [bootstrap]);

  // 选主题、重新扫描都按**偏好**模式来;单模式主题只改变生效模式,切回双模式主题时自然恢复
  const selectTheme = useCallback(
    (id: string) => applyWith(entriesRef.current, id, preferredMode),
    [applyWith, preferredMode],
  );
  const setMode = useCallback(
    (mode: ThemeMode) => applyWith(entriesRef.current, selectedId, mode),
    [applyWith, selectedId],
  );
  const reload = useCallback(
    () => bootstrap(selectedId, preferredMode),
    [bootstrap, selectedId, preferredMode],
  );

  const value = useMemo<ThemeContextValue>(
    () => ({
      theme,
      mode: theme.mode,
      preferredMode,
      selectedId,
      fallbackReason,
      entries,
      themesDir,
      listingError,
      loading,
      selectTheme,
      setMode,
      reload,
      resetSeq,
      agentColor: (id: string) => pickAgentColor(theme, id),
      chartPalette: theme.chart.palette,
    }),
    [
      theme,
      preferredMode,
      selectedId,
      fallbackReason,
      entries,
      themesDir,
      listingError,
      loading,
      selectTheme,
      setMode,
      reload,
      resetSeq,
    ],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

/** 没有 Provider 时退回默认主题(便于单独渲染组件) */
export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (ctx) return ctx;
  const theme = initialTheme();
  const noop = (): ApplyOutcome => ({
    id: theme.id,
    mode: theme.mode,
    preferredMode: theme.mode,
    fallbackReason: null,
  });
  return {
    theme,
    mode: theme.mode,
    preferredMode: theme.mode,
    selectedId: theme.id,
    fallbackReason: null,
    entries: builtinEntries(),
    themesDir: null,
    listingError: null,
    loading: false,
    selectTheme: noop,
    setMode: noop,
    reload: async () => undefined,
    resetSeq: 0,
    agentColor: (id: string) => pickAgentColor(theme, id),
    chartPalette: theme.chart.palette,
  };
}
