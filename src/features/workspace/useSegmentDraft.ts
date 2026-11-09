import { useEffect, useMemo, useRef, useState } from "react";
import type { InspectionCoverage, WorkspacePlanView } from "../plan/types";
import { errorText } from "../../lib/errors";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import type { SegmentSaveImpact, WorkspaceView } from "./types";
import { initialDraft, passEnds, piecePasses, segmentsForSave, spans, structureChanged, valid, type WorkbenchDraft } from "./workbenchDraft";

const DEBOUNCE_MS = 700;

export type SegmentSaveStatus = "saved" | "pending" | "saving" | "failed" | "confirm" | "invalid";
export const segmentSaveText: Record<SegmentSaveStatus, string> = {
  saved: "分段已保存", pending: "分段等待保存", saving: "分段保存中", failed: "分段保存失败", confirm: "分段待确认", invalid: "分段未保存",
};

/**
 * 示教件上的分段草稿（段 = 笔内的连续帧组，D-SB-44）：草稿始终含全部笔末边界；改动防抖后先预演，
 * 会清掉已示教走廊（impact.dropped）时停下等确认，否则直接保存为段列表 + 示教件已确认边界。存下即确认。
 * 与段参数、走廊的自动保存共用修订号：它们有待存的改动时先等它们存完。
 */
export function useSegmentDraft(view: WorkspaceView, plan: WorkspacePlanView) {
  const { doc, act, busy, pendingSave, frameDirty, unsavable, navigationDirty, setError, segmentDraft, setSegmentDraft } = useWorkspace();
  const roundId = view.workspace.teachingPiece?.roundId ?? "";
  const groupCount = plan.pieces.find(piece => piece.roundId === roundId)?.groupCount ?? 0;
  const passes = piecePasses(view.workspace.teachingPiece?.passes, groupCount);
  const ends = passEnds(passes);
  const saved = view.workspace.captureReview?.rounds[roundId] ?? null;
  const passSkip = view.workspace.doc.passSkip;
  const structure = JSON.stringify([plan.segments.map(s => [s.kind, s.cameras, !!s.skip]), plan.cameras, saved?.cuts, saved?.status, passes, passSkip]);
  const baseline = useMemo(() => groupCount ? initialDraft(groupCount, plan.segments, plan.cameras, saved, passes, passSkip) : null, [groupCount, structure]);
  const stored = useMemo(() => {
    if (!groupCount || saved?.status !== "confirmed") return null;
    const draft = initialDraft(groupCount, plan.segments, plan.cameras, saved);
    return JSON.stringify(draft.cuts) === JSON.stringify(saved.cuts) ? draft : null;
  }, [groupCount, structure]);
  const [edit, setEdit] = useState<{ roundId: string; draft: WorkbenchDraft } | null>(segmentDraft);
  useEffect(() => { setEdit(segmentDraft); }, [segmentDraft]);
  const draft = edit?.roundId === roundId ? edit.draft : baseline;
  const signature = JSON.stringify(draft);
  const dirty = !!draft && (!stored || signature !== JSON.stringify(stored));
  const restructured = !!draft && structureChanged(plan.segments, draft.kinds);
  const alignment = view.workspace.passAlignment;
  const locked = view.workspace.pending ? "候选版本正在等待生效"
    : alignment && !alignment.confirmed ? "换示教件的逐笔对照还没确认：先在上方确认对照"
      : navigationDirty ? "请先保存或撤销导航图修改"
        : unsavable ? "走廊有未保存的修改（按站距不足三站，不会自动保存）：改好或清空后再改分段" : "";
  const waiting = busy || pendingSave || frameDirty || !doc;

  const [prompt, setPrompt] = useState<{ signature: string; revision: number; impact: SegmentSaveImpact } | null>(null);
  const [working, setWorking] = useState(false);
  /** 最近一次预演的检测覆盖：有检测段应测帧数为 0 时提示（不拦保存） */
  const [coverage, setCoverage] = useState<InspectionCoverage | null>(null);
  const [retry, setRetry] = useState(0);
  const failed = useRef("");
  const done = useRef("");
  const latest = useRef({ signature, revision: view.workspace.revision, waiting });
  latest.current = { signature, revision: view.workspace.revision, waiting };
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  const commit = async (target: WorkbenchDraft, key: string) => {
    if (!doc) return;
    const segments = segmentsForSave(plan.segments, target, plan.segmentDefaults, plan.cameras);
    setWorking(true);
    const next = await act(() => workspaceApi.segmentTemplateSave(doc.id, latest.current.revision, segments, target.cuts), "");
    if (next) setSegmentDraft(current => current?.roundId === roundId && JSON.stringify(current.draft) === key ? null : current);
    if (!alive.current) return;
    setWorking(false);
    if (next) {
      done.current = key; failed.current = "";
      setPrompt(null);
      setEdit(current => current && JSON.stringify(current.draft) === key ? null : current);
    } else failed.current = key;
  };

  const ready = !!draft && dirty && valid(draft) && !locked && !waiting && !working;
  const prompting = prompt?.signature === signature && prompt.revision === view.workspace.revision;
  useEffect(() => {
    if (!ready || !draft || !doc || prompting || failed.current === signature || done.current === signature) return;
    const target = draft, key = signature;
    const timer = setTimeout(async () => {
      const segments = segmentsForSave(plan.segments, target, plan.segmentDefaults, plan.cameras);
      setWorking(true);
      let impact: SegmentSaveImpact;
      const revision = latest.current.revision;
      try { impact = await workspaceApi.segmentTemplatePreview(doc.id, revision, segments, target.cuts); }
      catch (e) {
        if (alive.current) { setWorking(false); failed.current = key; setError(errorText(e)); }
        return;
      }
      if (!alive.current) return;
      setWorking(false);
      setCoverage(impact.coverage);
      if (latest.current.signature !== key || latest.current.revision !== revision || latest.current.waiting) return;
      if (impact.dropped.length) { setPrompt({ signature: key, revision, impact }); return; }
      await commit(target, key);
    }, DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [ready, signature, prompting, retry, view.workspace.revision]);

  const status: SegmentSaveStatus = !dirty ? "saved" : prompting ? "confirm" : working ? "saving"
    : failed.current === signature ? "failed" : draft && !valid(draft) ? "invalid" : "pending";

  return {
    roundId, groupCount, passes, ends, draft, stored, dirty, restructured, locked, status,
    spans: draft ? spans(draft) : [],
    edited: !!edit && edit.roundId === roundId,
    impact: prompting ? prompt!.impact : null,
    coverage,
    change: (next: WorkbenchDraft) => {
      if (locked) return;
      const edit = stored && JSON.stringify(next) === JSON.stringify(stored) ? null : { roundId, draft: next };
      setEdit(edit); setSegmentDraft(edit);
    },
    undo: () => { setEdit(null); setSegmentDraft(null); setPrompt(null); },
    confirm: () => { if (draft && prompting && !waiting && !working) void commit(draft, signature); },
    retry: () => { failed.current = ""; setRetry(n => n + 1); },
  };
}

export type SegmentDraft = ReturnType<typeof useSegmentDraft>;
