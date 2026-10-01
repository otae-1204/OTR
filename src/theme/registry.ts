/**
 * 主题发现:内置主题 + 用户主题目录(Rust `list_themes` 命令枚举并读取文件,
 * 前端负责解析与校验)。两者进入同一份 ThemeEntry 列表。
 */

import { api } from "../api/bindings";
import { BUILTIN_THEMES } from "./builtin";
import { parseThemeFile } from "./validate";
import type { ThemeEntry, ThemeFile, ThemeMode } from "./types";

export interface ThemeListing {
  entries: ThemeEntry[];
  /** 用户主题目录;拿不到时为 null */
  themesDir: string | null;
  /** 枚举目录本身失败(不是单个文件失败)时的原因 */
  error: string | null;
}

function modesOf(entry: Pick<ThemeEntry, "manifest">): ThemeMode[] {
  const m = entry.manifest?.modes ?? {};
  const out: ThemeMode[] = [];
  if (m.dark) out.push("dark");
  if (m.light) out.push("light");
  return out;
}

export function builtinEntries(): ThemeEntry[] {
  return BUILTIN_THEMES.map((manifest) => {
    const e: ThemeEntry = {
      id: manifest.id,
      name: manifest.name,
      source: "builtin",
      path: null,
      manifest,
      diagnostics: [],
      modes: [],
    };
    e.modes = modesOf(e);
    return e;
  });
}

/** 把 Rust 读回来的文件列表变成 ThemeEntry;重复 id 的后来者作废 */
export function entriesFromFiles(files: ThemeFile[], reservedIds: readonly string[]): ThemeEntry[] {
  const seen = new Set<string>(reservedIds);
  const out: ThemeEntry[] = [];
  for (const f of files) {
    const entry: ThemeEntry = {
      id: f.name,
      name: f.name,
      source: "user",
      path: f.path,
      manifest: null,
      diagnostics: [],
      modes: [],
    };
    if (f.error != null || f.contents == null) {
      entry.diagnostics.push({ level: "error", path: "", message: f.error ?? "读取失败" });
      out.push(entry);
      continue;
    }
    const result = parseThemeFile(f.contents, { reservedIds });
    entry.diagnostics = result.diagnostics;
    if (!result.manifest) {
      const reserved = result.diagnostics.some(
        (d) => d.path === "id" && d.message.includes("内置主题的 id"),
      );
      // 用户目录里放了一份同名文件时,让位给内置那条,不再另挂一条无效项
      if (reserved) continue;
      out.push(entry);
      continue;
    }
    if (result.manifest) {
      if (seen.has(result.manifest.id)) {
        entry.diagnostics.push({
          level: "error",
          path: "id",
          message: `id「${result.manifest.id}」与另一个主题重复,本文件已忽略`,
        });
      } else {
        seen.add(result.manifest.id);
        entry.manifest = result.manifest;
        entry.id = result.manifest.id;
        entry.name = result.manifest.name;
        entry.modes = modesOf(entry);
      }
    }
    out.push(entry);
  }
  return out;
}

export async function loadThemeListing(): Promise<ThemeListing> {
  const builtins = builtinEntries();
  const reserved = builtins.map((b) => b.id);
  let themesDir: string | null = null;
  let error: string | null = null;
  let users: ThemeEntry[] = [];
  try {
    themesDir = await api.getThemesDir();
  } catch (err) {
    console.error("[theme] getThemesDir 失败", err);
  }
  try {
    const files = await api.listThemes();
    users = entriesFromFiles(files, reserved);
  } catch (err) {
    console.error("[theme] listThemes 失败", err);
    error = String(err);
  }
  return { entries: [...builtins, ...users], themesDir, error };
}
