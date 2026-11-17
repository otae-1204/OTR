import dshPng from "../assets/agents/dsh.png";
import zcodePng from "../assets/agents/zcode.png";
import antigravityMark from "../assets/agents/antigravity.svg";
import grokDark from "../assets/agents/grok-logomark-dark.svg";
import grokLight from "../assets/agents/grok-logomark-light.svg";
import { createContext, useContext } from "react";
import { useTheme } from "../theme/ThemeProvider";
import {
  ClaudeMarkIcon,
  CursorMarkIcon,
  OpenAIMarkIcon,
  OpenCodeMarkIcon,
  PiMarkIcon,
} from "./icons";

/** 使用位图图标的 Agent(官方 logo 资源) */
const IMAGE_ICONS: Record<string, string> = {
  dsh: dshPng,
  zcode: zcodePng,
};

/** id → data URL。用户指定的图标优先于内置图标。 */
export const AgentIconOverrides = createContext<Record<string, string>>({});

export function usesImageIcon(id: string): boolean {
  return id in IMAGE_ICONS || id === "grok";
}

interface AgentIconProps {
  id: string;
  /** 该 Agent 的品牌色(fill 图标与字母兜底用) */
  color: string;
  className?: string;
}

/** Agent 品牌图标:用户指定的图片优先,其次是内置品牌标。 */
export function AgentIcon({ id, color, className }: AgentIconProps) {
  const { mode } = useTheme();
  const overrides = useContext(AgentIconOverrides);
  const custom = overrides[id];
  const cls = className ?? "h-5 w-5";
  if (custom) {
    return (
      <img
        src={custom}
        alt=""
        draggable={false}
        className={`h-full w-full object-contain ${className ?? ""}`}
      />
    );
  }
  if (id === "grok") {
    return (
      <img
        src={mode === "dark" ? grokLight : grokDark}
        alt="Grok"
        draggable={false}
        className={`object-contain ${cls}`}
      />
    );
  }
  if (id === "antigravity") {
    return (
      <img
        src={antigravityMark}
        alt="Antigravity"
        draggable={false}
        className={`object-contain ${cls}`}
      />
    );
  }
  const img = IMAGE_ICONS[id];
  if (img) {
    return (
      <img
        src={img}
        alt={id}
        draggable={false}
        className={`h-full w-full object-contain ${id === "zcode" ? "p-1" : "p-0.5"} ${className ?? ""}`}
      />
    );
  }
  switch (id) {
    case "claude-code":
      return <ClaudeMarkIcon className={cls} style={{ color }} />;
    case "codex":
      return <OpenAIMarkIcon className={cls} style={{ color }} />;
    case "opencode":
      return <OpenCodeMarkIcon className={cls} style={{ color }} />;
    case "cursor":
      return <CursorMarkIcon className={cls} style={{ color }} />;
    case "pi":
      return (
        <PiMarkIcon
          className={cls}
          style={{ color }}
          viewBox="0 0 800 800"
        />
      );
    default:
      return (
        <span className="text-sm font-bold leading-none" style={{ color }}>
          {(id.charAt(0) || "?").toUpperCase()}
        </span>
      );
  }
}
