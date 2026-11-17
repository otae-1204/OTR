import { useState } from "react";
import { api, type Settings as AppSettings } from "../../api/bindings";
import { compareVersions, fetchLatestVersion } from "../../lib/remote";
import { useTheme } from "../../theme/ThemeProvider";
import type { ThemeEntry, ThemeMode } from "../../theme/types";
import type { ConfirmOptions } from "../ConfirmDialog";
import { SelectMenu } from "../SelectMenu";
import { DatabaseIcon, MoonIcon, RefreshIcon, SunIcon } from "../icons";
import { SectionCard, Toggle } from "./SettingsUI";

export function AppearanceSection({
  settings,
  setSettings,
  persist,
  onDataChanged,
  appVersion,
  updateLatest,
  confirm,
}: {
  settings: AppSettings | null;
  setSettings: (next: AppSettings) => void;
  persist: (next: AppSettings) => Promise<void>;
  onDataChanged: () => void;
  appVersion: string;
  updateLatest: string | null;
  confirm: (options: ConfirmOptions) => Promise<boolean>;
}) {
  const themeCtx = useTheme();
  const theme = themeCtx.mode;
  const [themeReloading, setThemeReloading] = useState(false);
  const [busyAction, setBusyAction] = useState<"none" | "refresh" | "rescan">("none");
  const [updateState, setUpdateState] = useState<
    "idle" | "checking" | "latest" | "available" | "error"
  >("idle");
  const [updateMsg, setUpdateMsg] = useState("");

  const applyTheme = (mode: ThemeMode) => {
    const outcome = themeCtx.setMode(mode);
    if (settings) {
      const next = { ...settings, preferredMode: outcome.preferredMode, theme: outcome.mode };
      setSettings(next);
      void api.saveSettings(next).catch(() => undefined);
    }
  };

  const applyThemeId = (id: string) => {
    const outcome = themeCtx.selectTheme(id);
    if (settings) {
      const next = {
        ...settings,
        themeId: id,
        preferredMode: outcome.preferredMode,
        theme: outcome.mode,
      };
      setSettings(next);
      void api.saveSettings(next).catch(() => undefined);
    }
  };

  const reloadThemes = async () => {
    setThemeReloading(true);
    try {
      await themeCtx.reload();
    } finally {
      setThemeReloading(false);
    }
  };

  const activeEntry: ThemeEntry | undefined = themeCtx.entries.find(
    (e) => e.id === themeCtx.theme.id && e.manifest,
  );
  const singleMode: ThemeMode | null =
    activeEntry && activeEntry.modes.length === 1 ? activeEntry.modes[0] : null;
  const modeLabel = (m: ThemeMode) => (m === "dark" ? "暗色" : "亮色");
  const modeLockedTip = singleMode
    ? `当前主题只提供${modeLabel(singleMode)};换回双模式主题后可修改深浅偏好`
    : undefined;
  const selectedExists = themeCtx.entries.some(
    (e) => e.id === themeCtx.selectedId && e.manifest,
  );
  const themeIssues = themeCtx.entries.filter(
    (e) => e.source === "user" && e.diagnostics.length > 0,
  );

  const checkUpdate = async () => {
    setUpdateState("checking");
    setUpdateMsg("正在从 GitHub Releases 获取最新版本…");
    try {
      const latest = await fetchLatestVersion();
      if (!latest) {
        setUpdateState("error");
        setUpdateMsg("获取失败:网络无法访问 GitHub,稍后再试");
        return;
      }
      if (updateLatest && updateLatest !== latest) {
        onDataChanged();
      }
      const cur = appVersion || "0.0.0";
      if (latest === cur) {
        setUpdateState("latest");
        setUpdateMsg(`已是最新版本 v${cur}`);
      } else if (compareVersions(latest, cur) > 0) {
        setUpdateState("available");
        setUpdateMsg(`发现新版本 v${latest}(当前 v${cur}),前往 Releases 页下载`);
      } else {
        setUpdateState("latest");
        setUpdateMsg(`本地 v${cur} 比 Releases 上的 v${latest} 还新(开发版?)`);
      }
    } catch {
      setUpdateState("error");
      setUpdateMsg("获取失败,请检查网络");
    }
  };

  const handleRefresh = async () => {
    setBusyAction("refresh");
    try {
      await api.rescan(false);
      onDataChanged();
    } catch (err) {
      console.error("[Settings] rescan 失败", err);
    } finally {
      setBusyAction("none");
    }
  };

  const handleFullRescan = async () => {
    const ok = await confirm({
      title: "确定要全量重扫吗？",
      description: "将清除本地缓存并重新解析所有本地记录，耗时取决于历史日志体积。",
      confirmLabel: "继续重扫",
      tone: "destructive",
    });
    if (!ok) return;
    setBusyAction("rescan");
    try {
      await api.rescan(true);
      onDataChanged();
    } catch (err) {
      console.error("[Settings] 全量重扫失败", err);
    } finally {
      setBusyAction("none");
    }
  };

  return (
    <div className="space-y-4">
      <SectionCard
        icon={<SunIcon className="h-4 w-4 text-primary" />}
        title="外观与启动"
      >
        <div className="px-4 py-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="min-w-0">
              <div className="text-sm font-medium">主题</div>
              <div className="mt-0.5 truncate text-xs text-muted-foreground">
                {themeCtx.fallbackReason ?? `当前:${themeCtx.theme.name}`}
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <SelectMenu
                value={selectedExists ? themeCtx.selectedId : "__missing__"}
                onChange={applyThemeId}
                disabled={!settings}
                title="选择主题;第三方主题放进下方目录后点「重新扫描」"
                ariaLabel="主题"
                className="h-8 max-w-[220px]"
                options={[
                  ...(selectedExists
                    ? []
                    : [
                        {
                          value: "__missing__",
                          label: `${themeCtx.selectedId}(未找到)`,
                          disabled: true,
                        },
                      ]),
                  ...themeCtx.entries.map((e) => ({
                    value: e.manifest ? e.id : `__invalid__:${e.path ?? e.id}`,
                    label: `${e.name}${e.source === "builtin" ? " · 内置" : ""}${
                      e.manifest ? "" : "(无效)"
                    }`,
                    disabled: !e.manifest,
                  })),
                ]}
              />
              <button
                type="button"
                onClick={() => void reloadThemes()}
                disabled={themeReloading}
                title="重新扫描主题目录"
                data-theme-part="button"
                className="inline-flex h-8 items-center gap-1 rounded-lg border border-border bg-background px-2.5 text-xs font-medium transition-colors hover:bg-overlay/5 disabled:opacity-50"
              >
                <RefreshIcon
                  className={`h-3.5 w-3.5 ${themeReloading ? "animate-spin" : ""}`}
                />
                重新扫描
              </button>
            </div>
          </div>
          <p className="mt-2 break-all text-11px text-muted-foreground">
            第三方主题:把 <code className="font-mono">theme.json</code> 放进{" "}
            <code className="font-mono">{themeCtx.themesDir ?? "(应用数据目录)/themes"}</code>
            {" "}后重新扫描;格式见 docs/theme_interface.md
          </p>
          {themeCtx.listingError ? (
            <p className="mt-1 text-11px text-danger-text">
              主题目录读取失败:{themeCtx.listingError}
            </p>
          ) : null}
          {themeIssues.length > 0 ? (
            <ul className="mt-2 space-y-1 text-11px">
              {themeIssues.map((e) => (
                <li key={e.path ?? e.id}>
                  <span className="font-medium">{e.name}</span>
                  <span className="text-muted-foreground">({e.path})</span>
                  <ul className="mt-0.5 space-y-0.5 pl-3">
                    {e.diagnostics.slice(0, 8).map((d, i) => (
                      <li
                        key={i}
                        className={
                          d.level === "error" ? "text-danger-text" : "text-warning-text"
                        }
                      >
                        {d.level === "error" ? "错误" : "警告"}
                        {d.path ? ` · ${d.path}` : ""}:{d.message}
                      </li>
                    ))}
                    {e.diagnostics.length > 8 ? (
                      <li className="text-muted-foreground">
                        …还有 {e.diagnostics.length - 8} 条
                      </li>
                    ) : null}
                  </ul>
                </li>
              ))}
            </ul>
          ) : null}
        </div>

        <div className="flex items-center justify-between border-t border-border/40 px-4 py-3">
          <div>
            <div className="text-sm font-medium">深浅模式</div>
            <div className="mt-0.5 text-xs text-muted-foreground">
              当前:{modeLabel(theme)}
              {singleMode
                ? themeCtx.preferredMode !== singleMode
                  ? `(该主题只提供${modeLabel(singleMode)};你偏好的${modeLabel(themeCtx.preferredMode)}会在换回双模式主题时恢复)`
                  : `(该主题只提供${modeLabel(singleMode)})`
                : ""}
            </div>
          </div>
          <div data-theme-part="segmented" className="flex items-center gap-1 rounded-xl bg-muted p-1">
            <button
              type="button"
              onClick={() => applyTheme("dark")}
              disabled={singleMode !== null || !settings}
              title={modeLockedTip}
              data-theme-part="segmented-button"
              data-theme-state={theme === "dark" ? "selected" : undefined}
              className={`flex h-7 items-center gap-1 rounded-lg px-2.5 text-xs font-medium transition-[color,background-color,box-shadow] duration-200 disabled:cursor-not-allowed disabled:opacity-40 ${
                theme === "dark"
                  ? "bg-background shadow-sm text-foreground"
                  : "text-muted-foreground hover:bg-overlay/5"
              }`}
            >
              <MoonIcon className="h-3.5 w-3.5" />
              暗色
            </button>
            <button
              type="button"
              onClick={() => applyTheme("light")}
              disabled={singleMode !== null || !settings}
              title={modeLockedTip}
              data-theme-part="segmented-button"
              data-theme-state={theme === "light" ? "selected" : undefined}
              className={`flex h-7 items-center gap-1 rounded-lg px-2.5 text-xs font-medium transition-[color,background-color,box-shadow] duration-200 disabled:cursor-not-allowed disabled:opacity-40 ${
                theme === "light"
                  ? "bg-background shadow-sm text-foreground"
                  : "text-muted-foreground hover:bg-overlay/5"
              }`}
            >
              <SunIcon className="h-3.5 w-3.5" />
              亮色
            </button>
          </div>
        </div>

        <div className="flex items-center justify-between border-t border-border/40 px-4 py-3">
          <div className="text-sm font-medium">启动时最小化到托盘</div>
          <Toggle
            checked={settings?.startMinimized ?? false}
            onChange={() =>
              settings &&
              void persist({
                ...settings,
                startMinimized: !settings.startMinimized,
              })
            }
            ariaLabel="启动时最小化到托盘"
          />
        </div>
      </SectionCard>

      <SectionCard
        icon={<RefreshIcon className="h-4 w-4 text-primary" />}
        title="版本与缓存"
      >
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/40 px-4 py-3">
          <div className="min-w-0">
            <div className="text-sm font-medium">
              当前版本 {appVersion || "0.1.0"}
              {updateLatest ? (
                <span data-theme-part="badge" className="ml-2 rounded-md bg-notice/15 px-1.5 py-0.5 text-xs font-medium text-notice">
                  可更新到 v{updateLatest}
                </span>
              ) : null}
            </div>
            {updateMsg ? (
              <div
                className={`mt-1 text-xs ${
                  updateState === "error" ? "text-destructive" : "text-muted-foreground"
                }`}
              >
                {updateMsg}
              </div>
            ) : null}
          </div>
          <button
            type="button"
            onClick={() => void checkUpdate()}
            disabled={updateState === "checking"}
            data-theme-part="button"
            className="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-xs font-medium transition-colors hover:bg-overlay/5 disabled:opacity-50"
          >
            <RefreshIcon
              className={`h-3.5 w-3.5 ${updateState === "checking" ? "animate-spin" : ""}`}
            />
            检测更新
          </button>
        </div>

        <div className="flex flex-wrap items-center gap-3 px-4 py-3">
          <button
            type="button"
            onClick={() => void handleRefresh()}
            disabled={busyAction !== "none"}
            data-theme-part="button"
            className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-xs font-medium transition-colors hover:bg-overlay/5 disabled:opacity-50"
          >
            <RefreshIcon
              className={`h-3.5 w-3.5 ${busyAction === "refresh" ? "animate-spin" : ""}`}
            />
            立即刷新
          </button>
          <button
            type="button"
            onClick={() => void handleFullRescan()}
            disabled={busyAction !== "none"}
            className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-destructive/40 bg-background px-3 text-xs font-medium text-destructive transition-colors hover:bg-destructive/10 disabled:opacity-50"
          >
            <DatabaseIcon className="h-3.5 w-3.5" />
            全量重扫(清缓存重建)
          </button>
          {busyAction !== "none" ? (
            <span className="text-xs text-muted-foreground">
              正在扫描,完成后数据自动更新…
            </span>
          ) : null}
        </div>
      </SectionCard>
    </div>
  );
}
