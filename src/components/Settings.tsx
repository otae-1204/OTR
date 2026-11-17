import { useEffect, useState, type ReactNode } from "react";
import {
  api,
  DEFAULT_LIMIT_PROVIDERS,
  LIMIT_PROVIDER_LABELS,
  type AgentStatus,
  type Settings as AppSettings,
} from "../api/bindings";
import { useTheme } from "../theme/ThemeProvider";
import { useConfirm } from "./ConfirmDialog";
import { CoinsIcon, DatabaseIcon, GaugeIcon, SunIcon } from "./icons";
import { AgentsSection } from "./settings/AgentsSection";
import { AppearanceSection } from "./settings/AppearanceSection";
import { LimitSources } from "./settings/LimitSources";
import { PricingSection } from "./settings/PricingSection";
import { SectionCard } from "./settings/SettingsUI";

type SettingsTab = "general" | "sources" | "pricing" | "limits";

const TAB_KEY = "token-show-settings-tab";

function readInitialTab(): SettingsTab {
  try {
    const v = localStorage.getItem(TAB_KEY);
    if (v === "general" || v === "sources" || v === "pricing" || v === "limits") {
      return v;
    }
  } catch {
    // 忽略存储读取失败
  }
  return "general";
}

interface SettingsProps {
  agents: AgentStatus[];
  /** 设置变更 / 手动扫描后通知父级刷新数据 */
  onDataChanged: () => void;
  appVersion: string;
  /** GitHub 检测到的新版本号,null 表示无更新 */
  updateLatest: string | null;
}

export function Settings({
  agents,
  onDataChanged,
  appVersion,
  updateLatest,
}: SettingsProps) {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const [tab, setTab] = useState<SettingsTab>(readInitialTab);
  const themeCtx = useTheme();
  const { confirm, dialog } = useConfirm();

  useEffect(() => {
    let active = true;
    api
      .getSettings()
      .then((s) => {
        if (active) setSettings(s);
      })
      .catch((err) => console.error("[Settings] getSettings 失败", err));
    api
      .listModels()
      .then((m) => {
        if (active) setModels(m);
      })
      .catch((err) => console.error("[Settings] listModels 失败", err));
    return () => {
      active = false;
    };
  }, []);

  // 托盘「恢复默认主题」由 Rust 直接改写设置文件;把本页快照里的外观字段同步过来,
  // 否则之后在本页保存别的设置时会把旧的 themeId 写回去
  const resetSeq = themeCtx.resetSeq;
  useEffect(() => {
    if (resetSeq === 0) return;
    let active = true;
    api
      .getSettings()
      .then((fresh) => {
        if (!active) return;
        setSettings((cur) =>
          cur
            ? {
                ...cur,
                themeId: fresh.themeId,
                theme: fresh.theme,
                preferredMode: fresh.preferredMode,
              }
            : fresh,
        );
      })
      .catch((err) => console.error("[Settings] getSettings 失败", err));
    return () => {
      active = false;
    };
  }, [resetSeq]);

  const persist = async (next: AppSettings) => {
    setSettings(next);
    try {
      await api.saveSettings(next);
    } catch (err) {
      console.error("[Settings] saveSettings 失败", err);
    }
    onDataChanged();
  };

  const selectTab = (next: SettingsTab) => {
    setTab(next);
    try {
      localStorage.setItem(TAB_KEY, next);
    } catch {
      // 写不进去就只在这次打开里生效
    }
  };

  const navItems: {
    id: SettingsTab;
    label: string;
    icon: ReactNode;
    hint?: string;
    dot?: boolean;
  }[] = [
    {
      id: "general",
      label: "外观与通用",
      icon: <SunIcon className="h-3.5 w-3.5 shrink-0" />,
      dot: !!updateLatest,
    },
    {
      id: "sources",
      label: "数据源",
      icon: <DatabaseIcon className="h-3.5 w-3.5 shrink-0" />,
      hint: settings ? String(settings.enabledAgents.length) : undefined,
    },
    {
      id: "pricing",
      label: "成本定价",
      icon: <CoinsIcon className="h-3.5 w-3.5 shrink-0" />,
      hint: models.length > 0 ? String(models.length) : undefined,
    },
    {
      id: "limits",
      label: "额度来源",
      icon: <GaugeIcon className="h-3.5 w-3.5 shrink-0" />,
      hint: settings
        ? String((settings.limitProviders ?? DEFAULT_LIMIT_PROVIDERS).length)
        : undefined,
    },
  ];

  return (
    <div className="space-y-4">
      <h2 data-theme-part="page-title" className="text-lg font-semibold">
        设置
      </h2>

      <div className="grid grid-cols-1 items-start gap-4 md:grid-cols-[11rem_minmax(0,1fr)] lg:grid-cols-[12rem_minmax(0,1fr)]">
        <aside
          data-theme-part="card settings-section"
          className="rounded-xl border border-border bg-card p-1.5 md:sticky md:top-20"
        >
          <nav
            aria-label="设置分类"
            className="flex flex-row flex-wrap gap-1 md:flex-col"
          >
            {navItems.map((item) => {
              const active = tab === item.id;
              return (
                <button
                  key={item.id}
                  type="button"
                  onClick={() => selectTab(item.id)}
                  className={`flex items-center justify-between gap-2 rounded-lg border px-2.5 py-2 text-left text-xs font-medium transition-[color,background-color,border-color] duration-200 md:w-full ${
                    active
                      ? "border-primary/40 bg-primary/15 text-foreground"
                      : "border-transparent text-muted-foreground hover:bg-overlay/5 hover:text-foreground"
                  }`}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <span className={active ? "text-primary" : "text-muted-foreground"}>
                      {item.icon}
                    </span>
                    <span className="truncate">{item.label}</span>
                    {item.dot ? (
                      <span
                        className="h-1.5 w-1.5 shrink-0 rounded-full bg-notice"
                        title={`发现新版本 v${updateLatest}`}
                      />
                    ) : null}
                  </span>
                  {item.hint !== undefined ? (
                    <span className="tabular-nums text-11px text-muted-foreground">
                      {item.hint}
                    </span>
                  ) : null}
                </button>
              );
            })}
          </nav>
        </aside>

        <div className="min-w-0">
          {/* 保留各分类挂载，切换分类时不丢失未保存的表单草稿 */}
          <div className={tab === "general" ? "space-y-4 animate-fade-in" : "hidden"}>
            <AppearanceSection
              settings={settings}
              setSettings={setSettings}
              persist={persist}
              onDataChanged={onDataChanged}
              appVersion={appVersion}
              updateLatest={updateLatest}
              confirm={confirm}
            />
          </div>

          <div className={tab === "sources" ? "space-y-4 animate-fade-in" : "hidden"}>
            <AgentsSection
              agents={agents}
              settings={settings}
              persist={persist}
              confirm={confirm}
            />
          </div>

          <div className={tab === "pricing" ? "space-y-4 animate-fade-in" : "hidden"}>
            <PricingSection
              settings={settings}
              models={models}
              setSettings={setSettings}
              persist={persist}
              confirm={confirm}
            />
          </div>

          <div className={tab === "limits" ? "space-y-4 animate-fade-in" : "hidden"}>
            <SectionCard
              icon={<GaugeIcon className="h-4 w-4 text-primary" />}
              title="额度来源"
              summary={
                settings
                  ? (settings.limitProviders ?? DEFAULT_LIMIT_PROVIDERS)
                      .map((p) => LIMIT_PROVIDER_LABELS[p] ?? p)
                      .join("、") || "全部关闭"
                  : undefined
              }
            >
              <LimitSources onChanged={onDataChanged} confirm={confirm} />
            </SectionCard>
          </div>
        </div>
      </div>

      {dialog}
    </div>
  );
}
