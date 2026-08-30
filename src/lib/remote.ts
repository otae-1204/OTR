/** 远程数据:GitHub 版本检测 + 实时汇率(前端直连,失败静默返回 null) */

const RELEASES_API = "https://api.github.com/repos/otae-1204/OTR/releases/latest";

/** GitHub Releases 最新版本号(去掉 v 前缀);API 被限时/不可达时回落 jsdelivr 镜像;都失败返回 null */
export async function fetchLatestVersion(): Promise<string | null> {
  // 源 1:GitHub API(可拿到正式 Release)
  try {
    const res = await fetch(RELEASES_API, {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (res.ok) {
      const j = (await res.json()) as { tag_name?: string };
      const tag = (j.tag_name ?? "").trim();
      if (tag) return tag.replace(/^v/i, "");
    }
  } catch {
    /* 回落到镜像 */
  }
  // 源 2:jsdelivr gh 镜像读默认分支的 package.json(国内网络通常可达)
  try {
    const res = await fetch("https://cdn.jsdelivr.net/gh/otae-1204/OTR/package.json");
    if (res.ok) {
      const j = (await res.json()) as { version?: string };
      const v = (j.version ?? "").trim();
      if (v) return v;
    }
  } catch {
    /* 都失败 */
  }
  return null;
}

/** 语义化版本比较:>0 表示 a 更新 */
export function compareVersions(a: string, b: string): number {
  const pa = a
    .split(/[.+-]/)
    .slice(0, 3)
    .map((x) => parseInt(x, 10) || 0);
  const pb = b
    .split(/[.+-]/)
    .slice(0, 3)
    .map((x) => parseInt(x, 10) || 0);
  for (let i = 0; i < 3; i++) {
    const da = pa[i] ?? 0;
    const db = pb[i] ?? 0;
    if (da !== db) return da - db;
  }
  return 0;
}

/** 实时 USD→CNY 汇率;er-api 主源,frankfurter 回落 */
export async function fetchUsdCnyRate(): Promise<{ rate: number; source: string } | null> {
  try {
    const r = await fetch("https://open.er-api.com/v6/latest/USD");
    const j = await r.json();
    const v = Number(j?.rates?.CNY);
    if (v > 0) return { rate: v, source: "open.er-api.com" };
  } catch {
    /* 回落到下一源 */
  }
  try {
    const r = await fetch("https://api.frankfurter.app/latest?from=USD&to=CNY");
    const j = await r.json();
    const v = Number(j?.rates?.CNY);
    if (v > 0) return { rate: v, source: "frankfurter.app" };
  } catch {
    /* 两源都失败 */
  }
  return null;
}
