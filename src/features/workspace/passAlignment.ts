import type { PathSegment } from "../plan/types";
import type { PassAlignment, PassChange, PassLink, PassPair } from "./types";
import { segmentCameras } from "./workbenchDraft";

type Paired = PassPair & { old: number; new: number };
const paired = (row: PassPair): row is Paired => row.old != null && row.new != null;

/** 回传后端的对应：只取对上的笔，没列出的旧笔即删除、新笔即新增。 */
export const linksOf = (rows: PassPair[]): PassLink[] =>
  rows.filter(paired).map(row => ({ old: row.old, new: row.new, changed: row.change === "changed" }));

export const sameLinks = (a: PassPair[], b: PassPair[]) => JSON.stringify(linksOf(a)) === JSON.stringify(linksOf(b));

/** 旧件第 old 笔所在的对应在“不变 / 改动”之间切换。 */
export const toggleChange = (rows: PassPair[], old: number): PassLink[] =>
  linksOf(rows).map(link => link.old === old ? { ...link, changed: !link.changed } : link);

/** 断开旧件第 old 笔所在的对应：旧笔变删除、新笔变新增。 */
export const unlink = (rows: PassPair[], old: number): PassLink[] => linksOf(rows).filter(link => link.old !== old);

/** 能与删除的旧笔 old 连成一对的新增新笔：连上后两件的笔仍保持先后顺序。 */
export function linkable(rows: PassPair[], old: number): number[] {
  const links = linksOf(rows);
  if (links.some(link => link.old === old) || !rows.some(row => row.old === old)) return [];
  return rows
    .filter(row => row.old == null && row.new != null)
    .map(row => row.new as number)
    .filter(next => links.every(link => (link.old < old) === (link.new < next)));
}

/** 把删除的旧笔与新增的新笔连成一对（默认算改动，需人确认不变）；会打乱顺序时为 null。 */
export function link(rows: PassPair[], old: number, next: number, changed = true): PassLink[] | null {
  if (!linkable(rows, old).includes(next)) return null;
  return [...linksOf(rows), { old, new: next, changed }].sort((a, b) => a.old - b.old);
}

export const countChanges = (rows: PassPair[]): Record<PassChange, number> => {
  const counts: Record<PassChange, number> = { same: 0, changed: 0, added: 0, removed: 0 };
  for (const row of rows) counts[row.change] += 1;
  return counts;
};

/** 对照表的一行：单笔，或连续不变的几笔折叠成一行（key 为首行的旧笔下标）。区间都是笔下标（从 0 起），含两端。 */
export type AlignmentLine =
  | { kind: "row"; row: PassPair }
  | { kind: "fold"; key: number; old: [number, number]; new: [number, number]; open: boolean };

/** 连续 min 行以上的“不变”折叠成一行；展开的仍给出折叠行（供收起），后面跟逐行。 */
export function foldRows(rows: PassPair[], expanded: ReadonlySet<number>, min = 3): AlignmentLine[] {
  const lines: AlignmentLine[] = [];
  for (let at = 0; at < rows.length;) {
    let end = at;
    while (end < rows.length && rows[end].change === "same" && paired(rows[end])) end += 1;
    if (end - at >= min) {
      const first = rows[at] as Paired, last = rows[end - 1] as Paired;
      const open = expanded.has(first.old);
      lines.push({ kind: "fold", key: first.old, old: [first.old, last.old], new: [first.new, last.new], open });
      if (open) lines.push(...rows.slice(at, end).map(row => ({ kind: "row" as const, row })));
      at = end;
    } else {
      lines.push({ kind: "row", row: rows[at] });
      at += 1;
    }
  }
  return lines;
}

/** 确认对照后一段还要人做什么：不检测的段为 null；没有沿用条目的段算新增。与后端发布前检查的“按笔对照”同口径。 */
export interface CarryState { change: "same" | "changed" | "added"; cameras: string[]; missing: string[]; accepted: boolean; done: boolean }
export function carryOf(alignment: PassAlignment, segment: PathSegment, planCameras: string[]): CarryState | null {
  const cameras = segmentCameras(segment, planCameras);
  if (!cameras.length) return null;
  const carry = segment.id ? alignment.carry[segment.id] : undefined;
  const change = carry?.change ?? "added";
  const missing = cameras.filter(camera => !carry?.retaught?.includes(camera));
  const accepted = change === "same" && !!carry?.accepted;
  return { change, cameras, missing, accepted, done: !missing.length || accepted };
}
