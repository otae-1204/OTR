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
import { api } from "../api/bindings";
import { applyResolvedTheme, normalizeMode, readStoredMode } from "./apply";
import { OTR_THEME } from "./builtin";
import { loadThemeListing, builtinEntries } from "./registry";
import { pickAgentColor, resolveTheme } from "./resolve";
import {
  DEFAULT_THEME_ID,
  type ResolvedTheme,
  type ThemeEntry,
  type ThemeMode,
} from "./types";

export interface ApplyOutcome {
  /** 实际生效的主题 id(选中的主题不可用时是默认主题) */
  id: string;
  /** 实际生效的模式(主题不支持所选模式时会切到它支持的模式) */
  mode: ThemeMode;
  /** 没能按所选应用时的原因;正常时为 null */
  fallbackReason: string | null;
}

export interface ThemeContextValue {
  /** 当前已应用的主题 */
  theme: ResolvedTheme;
  mode: ThemeMode;
  /** 设置里记的主题 id(可能已不存在,见 fallbackReason) */
  selectedId: string;
  fallbackReason: string | null;
  entries: ThemeEntry[];
  themesDir: string | null;
  listingError: string | null;
  loading: boolean;
  /** 应用某个主题(不负责持久化;调用方自己把 id / mode 存进设置) */
  selectTheme: (id: string) => ApplyOutcome;
  /** 切换深浅模式(不负责持久化) */
  setMode: (mode: ThemeMode) => ApplyOutcome;
  /** 重新扫描主题目录,并按当前选择重新应用 */
  reload: () => Promise<void>;
  agentColor: (id: string) => string;
  chartPalette: string[];
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

function initialTheme(): ResolvedTheme {
  return resolveTheme(OTR_THEME, readStoredMode(), "builtin");
}

/**
 * 决定「选了 id、想要 mode」时实际应用什么。
 * - id 找不到 / 校验失败 → 默认主题 + 原因
 * - 主题不支持 mode → 用它支持的第一个模式 + 原因
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
    reason = `主题「${entry.name}」没有${mode === "dark" ? "暗色" : "亮色"}模式,已切到${useMode === "dark" ? "暗色" : "亮色"}`;
  }
  return {
    theme: resolveTheme(entry.manifest, useMode, entry.source),
    outcome: { id: entry.id, mode: useMode, fallbackReason: reason },
  };
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setTheme] = useState<ResolvedTheme>(initialTheme);
  const [entries, setEntries] = useState<ThemeEntry[]>(builtinEntries);
  const [themesDir, setThemesDir] = useState<string | null>(null);
  const [listingError, setListingError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string>(DEFAULT_THEME_ID);
  const [fallbackReason, setFallbackReason] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const entriesRef = useRef(entries);
  entriesRef.current = entries;

  const applyWith = useCallback(
    (list: ThemeEntry[], id: string, mode: ThemeMode): ApplyOutcome => {
      const { theme: next, outcome } = decide(list, id, mode);
      try {
        applyResolvedTheme(next);
      } catch (err) {
        console.error("[theme] 应用主题失败", err);
      }
      setTheme(next);
      setSelectedId(id);
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
      // 设置文件是持久化的真相;localStorage 只是首帧缓存
      const mode = modeOverride ?? normalizeMode(settings?.theme) ?? readStoredMode();
      applyWith(listing.entries, id, mode);
      setLoading(false);
    },
    [applyWith],
  );

  useEffect(() => {
    void bootstrap();
  }, [bootstrap]);

  const selectTheme = useCallback(
    (id: string) => applyWith(entriesRef.current, id, theme.mode),
    [applyWith, theme.mode],
  );
  const setMode = useCallback(
    (mode: ThemeMode) => applyWith(entriesRef.current, selectedId, mode),
    [applyWith, selectedId],
  );
  const reload = useCallback(
    () => bootstrap(selectedId, theme.mode),
    [bootstrap, selectedId, theme.mode],
  );

  const value = useMemo<ThemeContextValue>(
    () => ({
      theme,
      mode: theme.mode,
      selectedId,
      fallbackReason,
      entries,
      themesDir,
      listingError,
      loading,
      selectTheme,
      setMode,
      reload,
      agentColor: (id: string) => pickAgentColor(theme, id),
      chartPalette: theme.chart.palette,
    }),
    [
      theme,
      selectedId,
      fallbackReason,
      entries,
      themesDir,
      listingError,
      loading,
      selectTheme,
      setMode,
      reload,
    ],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

/** 没有 Provider 时退回默认主题(便于单独渲染组件) */
export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (ctx) return ctx;
  const theme = initialTheme();
  const noop = (): ApplyOutcome => ({ id: theme.id, mode: theme.mode, fallbackReason: null });
  return {
    theme,
    mode: theme.mode,
    selectedId: theme.id,
    fallbackReason: null,
    entries: builtinEntries(),
    themesDir: null,
    listingError: null,
    loading: false,
    selectTheme: noop,
    setMode: noop,
    reload: async () => undefined,
    agentColor: (id: string) => pickAgentColor(theme, id),
    chartPalette: theme.chart.palette,
  };
}
