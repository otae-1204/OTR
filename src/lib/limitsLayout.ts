/**
 * 额度页的行排布。
 *
 * 一行是一个或两个账号。两个账号叠在同一行里只有一种情况:
 * 用户把两条「只有一列数据」的行拖到一起。多列行占满整行,不能并进去。
 * 这里不管具体是哪个窗口,只认账号 id 和「是不是单列」。
 */

export type DropMode = "before" | "after" | "merge";

export interface LayoutDrop {
  mode: DropMode;
  /** 落点那一行里的账号,顺序与屏幕上一致 */
  rowIds: string[];
}

/** 命中测试用的一行竖条。坐标是视口坐标,和 pointer 事件一致。 */
export interface RowBand {
  top: number;
  bottom: number;
  ids: string[];
}

const ROW_KEY_SEP = "|";

/** 账号 id 只含字母数字和横线,用 | 拼进 data 属性不会撞车 */
export function rowKey(ids: readonly string[]): string {
  return ids.join(ROW_KEY_SEP);
}

export function parseRowKey(value: string): string[] {
  if (!value) return [];
  return value.split(ROW_KEY_SEP).filter((id) => id.length > 0);
}

/**
 * 单列行中间这一段松手是合并,上下两端以及不能合并的行仍是插入。
 * 端头留 25%:行高大约 84px 时,插入带约 21px,比行间距更好瞄。
 */
const MERGE_EDGE = 0.25;

export function dropModeAt(ratio: number, mergeOk: boolean): DropMode {
  if (mergeOk && ratio >= MERGE_EDGE && ratio <= 1 - MERGE_EDGE) return "merge";
  return ratio < 0.5 ? "before" : "after";
}

/**
 * 指针在哪一行、该插入还是合并。
 *
 * 落在「正在拖的那一行」内部时返回 null:松手等于取消。
 * 这样从合并行里拖出一个账号时,手没离开这张卡片就不会被拆开。
 * 要拆开,把账号拖到别的行,或拖到这张卡片外面的缝里。
 */
export function hitTestBands(
  bands: readonly RowBand[],
  y: number,
  movingIds: readonly string[],
  single: ReadonlySet<string>,
): LayoutDrop | null {
  if (bands.length === 0) return null;
  const moving = new Set(movingIds);

  for (let i = 0; i < bands.length; i++) {
    const band = bands[i];
    if (y < band.top) return { mode: "before", rowIds: band.ids.slice() };
    if (y > band.bottom) continue;
    if (movingIds.length > 0 && movingIds.every((id) => band.ids.includes(id))) {
      return null;
    }
    const height = band.bottom - band.top;
    const ratio = height <= 0 ? 0.5 : (y - band.top) / height;
    const mergeOk =
      movingIds.length === 1 &&
      band.ids.length === 1 &&
      !moving.has(band.ids[0]) &&
      single.has(movingIds[0]) &&
      single.has(band.ids[0]);
    return { mode: dropModeAt(ratio, mergeOk), rowIds: band.ids.slice() };
  }

  const last = bands[bands.length - 1];
  return { mode: "after", rowIds: last.ids.slice() };
}

export function sameLayout(
  a: readonly (readonly string[])[],
  b: readonly (readonly string[])[],
): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i].length !== b[i].length) return false;
    for (let j = 0; j < a[i].length; j++) {
      if (a[i][j] !== b[i][j]) return false;
    }
  }
  return true;
}

function clone(rows: readonly (readonly string[])[]): string[][] {
  return rows.map((group) => [...group]);
}

/**
 * 把正在拖的账号放到落点。
 *
 * - merge:目标行留在左边,拖过去的账号接到右边。再对调就反过来拖一次。
 * - before / after:拖的是一整行(含已合并的一对)就整行挪走;拖的是一对里的一个,就先把那一个抽出来。
 * 落在自己身上(锚点也在被拖的集合里)时原样返回。
 */
export function applyLayout(
  rows: readonly (readonly string[])[],
  movingIdsIn: readonly string[],
  drop: LayoutDrop,
): string[][] {
  const present = new Set(rows.flat());
  const movingIds: string[] = [];
  const seenMoving = new Set<string>();
  for (const id of movingIdsIn) {
    if (!present.has(id) || seenMoving.has(id)) continue;
    seenMoving.add(id);
    movingIds.push(id);
  }
  if (movingIds.length === 0) return clone(rows);
  const moving = new Set(movingIds);

  if (drop.mode === "merge") {
    if (movingIds.length !== 1 || drop.rowIds.length !== 1) return clone(rows);
    const into = drop.rowIds[0];
    if (moving.has(into) || !present.has(into)) return clone(rows);
    const dragged = movingIds[0];
    let placed = false;
    const out: string[][] = [];
    for (const group of rows) {
      const rest = group.filter((id) => !moving.has(id));
      if (rest.length === 0) continue;
      if (!placed && rest.length === 1 && rest[0] === into) {
        placed = true;
        out.push([into, dragged]);
        continue;
      }
      out.push(rest);
    }
    return placed ? out : clone(rows);
  }

  const anchor = drop.rowIds.find((id) => !moving.has(id));
  if (!anchor) return clone(rows);

  const restRows: string[][] = [];
  for (const group of rows) {
    const rest = group.filter((id) => !moving.has(id));
    if (rest.length > 0) restRows.push(rest);
  }
  const index = restRows.findIndex((group) => group.includes(anchor));
  if (index < 0) return clone(rows);
  const at = drop.mode === "after" ? index + 1 : index;
  const next = restRows.slice();
  next.splice(at, 0, movingIds);
  return next;
}

/**
 * 用存下来的排布套到当前账号上。
 *
 * 已经不在的账号丢掉;不再是单列的合并拆回两行,顺序保留。
 * 存储里没出现的账号(新加的)按默认顺序接在最后,不插回原来的 provider 组,
 * 免得用户排好的顺序被一次刷新打乱。
 */
export function reconcileLayout(
  defaultOrder: readonly string[],
  saved: readonly (readonly string[])[],
  single: ReadonlySet<string>,
): string[][] {
  const known = new Set(defaultOrder);
  const used = new Set<string>();
  const out: string[][] = [];

  const take = (id: string): boolean => {
    if (!known.has(id) || used.has(id)) return false;
    used.add(id);
    out.push([id]);
    return true;
  };

  for (const group of saved) {
    const ids: string[] = [];
    const seen = new Set<string>();
    for (const id of group) {
      if (!known.has(id) || used.has(id) || seen.has(id)) continue;
      seen.add(id);
      ids.push(id);
    }
    if (ids.length >= 2 && single.has(ids[0]) && single.has(ids[1])) {
      used.add(ids[0]);
      used.add(ids[1]);
      out.push([ids[0], ids[1]]);
      for (const extra of ids.slice(2)) take(extra);
      continue;
    }
    for (const id of ids) take(id);
  }

  for (const id of defaultOrder) take(id);
  return out;
}

/** 读本地存的行序。格式不对就当没存过,回到默认顺序,不抛给界面。 */
export function parseStoredLayout(raw: string | null): string[][] | null {
  if (!raw) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!Array.isArray(parsed)) return null;
  const rows: string[][] = [];
  for (const group of parsed) {
    if (!Array.isArray(group)) continue;
    const ids: string[] = [];
    const seen = new Set<string>();
    for (const id of group) {
      if (typeof id !== "string" || id.length === 0 || seen.has(id)) continue;
      seen.add(id);
      ids.push(id);
    }
    if (ids.length === 1 || ids.length === 2) rows.push(ids);
    else if (ids.length > 2) {
      for (const id of ids) rows.push([id]);
    }
  }
  return rows.length > 0 ? rows : null;
}
