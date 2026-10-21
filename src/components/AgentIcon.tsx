import dshPng from "../assets/agents/dsh.png";
import zcodePng from "../assets/agents/zcode.png";
import grokDark from "../assets/agents/grok-logomark-dark.svg";
import grokLight from "../assets/agents/grok-logomark-light.svg";
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

export function usesImageIcon(id: string): boolean {
  return id in IMAGE_ICONS || id === "grok";
}

interface AgentIconProps {
  id: string;
  /** 该 Agent 的品牌色(fill 图标与字母兜底用) */
  color: string;
  className?: string;
}

/** Agent 品牌图标:知名品牌用官方 path/资源,其余字母兜底 */
export function AgentIcon({ id, color, className }: AgentIconProps) {
  const { mode } = useTheme();
  const cls = className ?? "h-5 w-5";
  if (id === "grok") {
    return (
      <img
        src={mode === "dark" ? grokLight : grokDark}
        alt="Grok"
        draggable={false}
        className={`h-full w-full object-contain p-0.5 ${className ?? ""}`}
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
        className={`h-full w-full object-contain p-0.5 ${className ?? ""}`}
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
        <span className={`text-sm font-bold leading-none ${cls ?? ""}`} style={{ color }}>
          {(id.charAt(0) || "?").toUpperCase()}
        </span>
      );
  }
}
