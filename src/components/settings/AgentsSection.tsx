import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AGENT_LABELS,
  api,
  type AgentStatus,
  type CustomAgentConfig,
  type Settings as AppSettings,
} from "../../api/bindings";
import { fmtTokens } from "../../lib/format";
import { useTheme } from "../../theme/ThemeProvider";
import { AgentIcon, usesImageIcon } from "../AgentIcon";
import type { ConfirmOptions } from "../ConfirmDialog";
import { SelectMenu } from "../SelectMenu";
import { DatabaseIcon, PlusIcon, TrashIcon } from "../icons";
import { Badge, SectionCard, Toggle } from "./SettingsUI";

const KNOWN_AGENTS = [
  "dsh",
  "claude-code",
  "codex",
  "antigravity",
  "zcode",
  "opencode",
  "pi",
  "cursor",
  "grok",
];

const KIND_OPTIONS: { value: string; label: string }[] = [
  { value: "claude-code", label: "Claude Code 布局" },
  { value: "codex", label: "Codex 布局" },
  { value: "zcode", label: "ZCode 布局" },
];

function kindLabel(kind: string): string {
  return KIND_OPTIONS.find((k) => k.value === kind)?.label ?? kind;
}

function slugify(name: string, existing: string[]): string {
  const base =
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "agent";
  let id = `custom-${base}`;
  let i = 2;
  while (existing.includes(id)) {
    id = `custom-${base}-${i}`;
    i++;
  }
  return id;
}

export function AgentsSection({
  agents,
  settings,
  persist,
  confirm,
}: {
  agents: AgentStatus[];
  settings: AppSettings | null;
  persist: (next: AppSettings) => Promise<void>;
  confirm: (options: ConfirmOptions) => Promise<boolean>;
}) {
  const themeCtx = useTheme();
  const [cName, setCName] = useState("");
  const [cKind, setCKind] = useState<CustomAgentConfig["kind"]>("claude-code");
  const [cDir, setCDir] = useState("");
  const [customErr, setCustomErr] = useState<string | null>(null);
  const [iconErr, setIconErr] = useState<string | null>(null);

  const chooseIcon = async (id: string, label: string) => {
    if (!settings) return;
    setIconErr(null);
    try {
      const selected = await open({
        multiple: false,
        title: `选择 ${label} 的图标`,
        filters: [
          {
            name: "图片",
            extensions: ["png", "jpg", "jpeg", "webp", "gif", "svg", "ico"],
          },
        ],
      });
      if (typeof selected !== "string") return;
      const path = await api.importAgentIcon(id, selected);
      await persist({
        ...settings,
        agentIcons: { ...(settings.agentIcons ?? {}), [id]: path },
      });
    } catch (err) {
      setIconErr(String(err));
    }
  };

  const resetIcon = async (id: string) => {
    if (!settings) return;
    setIconErr(null);
    try {
      await api.clearAgentIcon(id);
      const next = { ...(settings.agentIcons ?? {}) };
      delete next[id];
      await persist({ ...settings, agentIcons: next });
    } catch (err) {
      setIconErr(String(err));
    }
  };

  const toggleAgent = (id: string) => {
    if (!settings) return;
    const enabled = new Set(settings.enabledAgents);
    if (enabled.has(id)) {
      enabled.delete(id);
    } else {
      enabled.add(id);
    }
    void persist({ ...settings, enabledAgents: [...enabled] });
  };

  const addCustom = () => {
    if (!settings) return;
    const name = cName.trim();
    const dir = cDir.trim().replace(/[/\\]+$/, "");
    if (!name || !dir) {
      setCustomErr("请填写名称和数据目录");
      return;
    }
    if (
      settings.customAgents.some(
        (c) => c.dir.toLowerCase() === dir.toLowerCase(),
      )
    ) {
      setCustomErr("该目录已添加过");
      return;
    }
    setCustomErr(null);
    const cfg: CustomAgentConfig = {
      id: slugify(
        name,
        settings.customAgents.map((c) => c.id),
      ),
      name,
      kind: cKind,
      dir,
    };
    setCName("");
    setCDir("");
    void persist({
      ...settings,
      customAgents: [...settings.customAgents, cfg],
      enabledAgents: [...settings.enabledAgents, cfg.id],
    });
  };

  const removeCustom = async (id: string) => {
    if (!settings) return;
    const cfg = settings.customAgents.find((c) => c.id === id);
    if (!cfg) return;
    const ok = await confirm({
      title: `删除自定义 Agent「${cfg.name}」？`,
      description: "其已统计的历史数据会保留。",
      confirmLabel: "删除",
      tone: "destructive",
    });
    if (!ok) return;
    void persist({
      ...settings,
      customAgents: settings.customAgents.filter((c) => c.id !== id),
      enabledAgents: settings.enabledAgents.filter((a) => a !== id),
    });
  };

  const rows: {
    id: string;
    label: string;
    kindBadge?: string;
    dir?: string;
  }[] = KNOWN_AGENTS.map((id) => ({
    id,
    label: AGENT_LABELS[id] ?? id,
  }));
  for (const c of settings?.customAgents ?? []) {
    rows.push({ id: c.id, label: c.name, kindBadge: kindLabel(c.kind), dir: c.dir });
  }

  return (
    <div className="space-y-4">
      <SectionCard
        icon={<DatabaseIcon className="h-4 w-4 text-primary" />}
        title="数据源"
      >
        <div>
          {rows.map((row) => {
            const status = agents.find((a) => a.id === row.id);
            const detected = status?.detected ?? false;
            const enabled = settings
              ? settings.enabledAgents.includes(row.id)
              : (status?.enabled ?? false);
            const color = themeCtx.agentColor(row.id);
            const label = row.label;
            return (
              <div
                key={row.id}
                className="flex items-center justify-between border-b border-border/40 px-4 py-3 last:border-0"
              >
                <div className="flex min-w-0 items-center gap-3">
                  <button
                    type="button"
                    title="更换图标"
                    disabled={!settings}
                    onClick={() => void chooseIcon(row.id, label)}
                    className="flex h-9 w-9 shrink-0 items-center justify-center overflow-hidden rounded-lg border border-border/40 transition-colors hover:border-primary/60 disabled:opacity-50"
                    style={{ backgroundColor: `${color}1A`, color }}
                  >
                    <AgentIcon
                      id={row.id}
                      color={color}
                      className={
                        usesImageIcon(row.id) && row.id !== "grok"
                          ? "h-full w-full"
                          : "h-5 w-5"
                      }
                    />
                  </button>
                  <div className="min-w-0">
                    <div className="flex items-center gap-2 text-sm font-medium">
                      <span className="truncate">{label}</span>
                      {row.kindBadge ? (
                        <Badge tone="muted">{row.kindBadge}</Badge>
                      ) : null}
                    </div>
                    <div className="mt-1 flex flex-wrap items-center gap-1.5">
                      <Badge tone={detected ? "good" : "muted"}>
                        {detected ? "已检测到" : "未检测到"}
                      </Badge>
                      <Badge tone={enabled ? "good" : "muted"}>
                        {enabled ? "已启用" : "已停用"}
                      </Badge>
                      {status && status.totalTokens > 0 ? (
                        <span
                          className="text-xs text-muted-foreground"
                          title={`累计 ${status.totalTokens.toLocaleString()} tokens`}
                        >
                          累计 {fmtTokens(status.totalTokens)}
                        </span>
                      ) : null}
                    </div>
                    {row.dir ? (
                      <div
                        className="mt-1 max-w-[360px] truncate text-xs text-muted-foreground/70"
                        title={row.dir}
                      >
                        {row.dir}
                      </div>
                    ) : null}
                  </div>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  {settings?.agentIcons?.[row.id] ? (
                    <button
                      type="button"
                      onClick={() => void resetIcon(row.id)}
                      className="text-11px text-muted-foreground hover:text-foreground"
                    >
                      默认
                    </button>
                  ) : null}
                  <Toggle
                  checked={enabled}
                  disabled={!settings}
                  onChange={() => toggleAgent(row.id)}
                  ariaLabel={`${enabled ? "停用" : "启用"} ${label}`}
                />
                </div>
              </div>
            );
          })}
        </div>
        {iconErr ? <p className="px-4 pb-3 text-xs text-danger-text">{iconErr}</p> : null}
      </SectionCard>

      <SectionCard
        icon={<PlusIcon className="h-4 w-4 text-primary" />}
        title="自定义 Agent"
      >
        <div className="divide-y divide-border/40">
          {(settings?.customAgents ?? []).length === 0 ? (
            <p className="px-4 py-3 text-xs text-muted-foreground">
              还没有自定义 Agent,用下面的表单添加。
            </p>
          ) : (
            (settings?.customAgents ?? []).map((c) => (
              <div
                key={c.id}
                className="flex items-center justify-between px-4 py-2.5"
              >
                <div className="min-w-0">
                  <div className="flex items-center gap-2 text-sm font-medium">
                    <span className="truncate">{c.name}</span>
                    <Badge tone="muted">{kindLabel(c.kind)}</Badge>
                  </div>
                  <div
                    className="mt-0.5 max-w-[420px] truncate text-xs text-muted-foreground"
                    title={c.dir}
                  >
                    {c.dir}
                  </div>
                </div>
                <button
                  type="button"
                  onClick={() => void removeCustom(c.id)}
                  title="删除"
                  className="flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
                >
                  <TrashIcon className="h-4 w-4" />
                </button>
              </div>
            ))
          )}
        </div>
        <div className="space-y-2 border-t border-border/40 bg-background/40 p-4">
          <div className="grid gap-2 sm:grid-cols-3">
            <input
              value={cName}
              onChange={(e) => {
                setCName(e.target.value);
                if (customErr) setCustomErr(null);
              }}
              placeholder="名称,如 CodeBuddy"
              data-theme-part="input"
              className="h-8 rounded-lg border border-border bg-background px-2.5 text-xs outline-none focus:border-primary"
            />
            <SelectMenu
              value={cKind}
              onChange={(v) => setCKind(v as CustomAgentConfig["kind"])}
              ariaLabel="数据布局"
              className="h-8 w-full"
              options={KIND_OPTIONS}
            />
            <input
              value={cDir}
              onChange={(e) => {
                setCDir(e.target.value);
                if (customErr) setCustomErr(null);
              }}
              placeholder={"数据目录,如 C:\\Users\\you\\.codebuddy\\projects"}
              data-theme-part="input"
              className="h-8 rounded-lg border border-border bg-background px-2.5 text-xs outline-none focus:border-primary"
            />
          </div>
          <div className="flex items-center justify-between gap-2">
            {customErr ? (
              <span className="text-xs text-danger-text">{customErr}</span>
            ) : (
              <span />
            )}
            <button
              type="button"
              onClick={addCustom}
              disabled={!settings}
              data-theme-part="button-primary"
              className="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-50"
            >
              <PlusIcon className="h-3.5 w-3.5" />
              添加
            </button>
          </div>
        </div>
      </SectionCard>
    </div>
  );
}
