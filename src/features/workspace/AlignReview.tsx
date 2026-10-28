import { useEffect, useState, type KeyboardEvent } from "react";
import { Button } from "qing-steel";
import type { PieceView, WorkspacePlanView } from "../plan/types";
import { errorText } from "../../lib/errors";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, Notice } from "./components";
import { pieceLabel } from "./pieces";
import { ignoreKey, stripGroups, Thumb, useThumbs } from "./thumbs";
import { moveCut, segmentCameras } from "./workbenchDraft";
import type { AlignReviewReason, AlignSuggestion, CalibrationSyncResult } from "./types";
import type { ThumbStrip } from "./workbenchDraft";
import "./segments.css";

const reasonText: Record<AlignReviewReason, string> = { moved: "为保持递增被挪动", deviates: "偏离比例换算", unclear: "对齐不清晰" };

/** 缩略图宽度按显示的相机数放大：一台 160 px（缩略图原尺寸），两台 112 px，三台 72 px。 */
const cellWidth = (cameras: number) => (cameras <= 1 ? 160 : cameras === 2 ? 112 : 72);

/** 一侧（示教件或本件）边界两侧各两帧：边界两侧段的检测相机各一行，竖线标在边界处。 */
function BoundaryFrames({ label, strips, cameras, cut }: { label: string; strips: Record<string, ThumbStrip>; cameras: string[]; cut: number }) {
  const groups = [cut - 1, cut, cut + 1, cut + 2];
  const width = cellWidth(cameras.length);
  return <figure className="alr-side" aria-label={`${label}：帧组 ${cut} 之后`} style={{ ["--alr-cell" as string]: `${width}px` }}>
    <figcaption>{label} · {cut} | {cut + 1}</figcaption>
    {cameras.map(camera => {
      const strip = strips[camera];
      return <div key={camera} className="alr-row" title={camera}>
        {groups.map(group => <span key={group} className={`alr-cell${group === cut ? " edge" : ""}`} title={`${camera} 帧组 ${group}`}>
          {group >= 1 && strip && group <= strip.frames.length
            ? <Thumb pixels={strip.frames[group - 1]} width={strip.width} height={strip.height} />
            : <span className="swb-missing">—</span>}
        </span>)}
      </div>;
    })}
  </figure>;
}

/**
 * 一件标定件的边界复核（D-SB-51，多轮标定页）：软件把示教件的边界按图像对齐到这一件，每个边界一行并排显示示教件与本件边界两侧的帧；
 * 可信边界默认已勾选并收起，只有需重点复核的边界逐个确认（← → 微调、不越过相邻边界，↑ ↓ 换行，Enter 确认当前行）。
 * 各行都确认后一次保存并确认本件边界，再同步标定件把它加入标定。
 */
export default function AlignReview({ plan, piece, locked, onSynced }: {
  plan: WorkspacePlanView; piece: PieceView; locked: string; onSynced: (pieces: CalibrationSyncResult["pieces"]) => void;
}) {
  const { data, doc, act, busy } = useWorkspace();
  const roundId = piece.roundId;
  const [suggestion, setSuggestion] = useState<{ round: string; value: AlignSuggestion | null; error: string } | null>(null);
  const [cuts, setCuts] = useState<number[]>([]);
  const [checked, setChecked] = useState<boolean[]>([]);
  const [row, setRow] = useState(0);
  const review = data?.workspace.captureReview;
  const teachingRound = data?.workspace.teachingPiece?.roundId ?? "";
  const docId = doc?.id ?? "";
  const frozen = !!data?.workspace.pending;

  useEffect(() => {
    if (!docId || !teachingRound) return;
    let alive = true;
    setSuggestion(null);
    workspaceApi.captureReviewSuggest(docId, roundId)
      .then(value => {
        if (!alive) return;
        setSuggestion({ round: roundId, value, error: "" });
        const saved = review?.rounds[roundId];
        const confirmed = saved?.status === "confirmed" && saved.cuts.length === value.cuts.length;
        setCuts(confirmed ? saved.cuts : value.cuts.map(item => item.cut));
        // 可信边界默认已勾选，只有需重点复核的边界要逐个确认
        setChecked(value.cuts.map(item => confirmed || !item.review));
        const first = value.cuts.findIndex(item => !confirmed && item.review);
        setRow(Math.max(0, first));
      })
      .catch(e => alive && setSuggestion({ round: roundId, value: null, error: errorText(e) }));
    return () => { alive = false; };
  }, [roundId, docId, teachingRound]);

  const templateThumbs = useThumbs(teachingRound, plan.cameras);
  const roundThumbs = useThumbs(roundId, plan.cameras);
  const groupCount = stripGroups(roundThumbs.strips);

  const current = suggestion?.round === roundId ? suggestion : null;
  const value = current?.value ?? null;
  const ready = !!value && !!templateThumbs.strips && !!roundThumbs.strips && cuts.length === value.cuts.length;
  const nudge = (at: number, delta: number) => {
    const next = moveCut({ groupCount, cuts, kinds: [], cameras: [] }, at, cuts[at] + delta);
    if (next.cuts !== cuts) { setCuts(next.cuts); setChecked(list => list.map((item, i) => (i === at ? false : item))); }
  };
  const confirmRow = (at: number) => {
    const next = checked.map((item, i) => (i === at ? true : item));
    setChecked(next);
    const pending = next.findIndex(item => !item);
    setRow(pending >= 0 ? pending : Math.min(cuts.length - 1, at + 1));
  };
  const allChecked = checked.every(Boolean);
  const blocked = !ready || !allChecked || busy || frozen || !!locked || !doc || !data;
  const save = async () => {
    if (!doc || !data) return;
    const next = await act(() => workspaceApi.captureReviewConfirmCuts(doc.id, data.workspace.revision, roundId, cuts), "本件边界已确认");
    if (!next) return;
    let result: CalibrationSyncResult["pieces"] = [];
    const synced = await act(async () => {
      const response = await workspaceApi.calibrationSync(doc.id, next.workspace.revision);
      result = response.pieces;
      return response.view;
    }, "已同步标定件");
    if (synced) onSynced(result);
  };
  const onKey = (e: KeyboardEvent) => {
    if (!ready || ignoreKey(e)) return;
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") { e.preventDefault(); nudge(row, e.key === "ArrowLeft" ? -1 : 1); }
    else if (e.key === "ArrowUp" || e.key === "ArrowDown") { e.preventDefault(); setRow(Math.min(cuts.length - 1, Math.max(0, row + (e.key === "ArrowUp" ? -1 : 1)))); }
    else if (e.key === "Enter") { e.preventDefault(); confirmRow(row); }
  };

  return <section className="alr" aria-label={`边界复核 · ${pieceLabel(piece)}`}>
    <p className="muted hint">软件按图像把示教件的边界对齐到这一件；对齐可信的边界默认已勾选，只需逐个确认标“需重点复核”的边界（← → 微调、↑ ↓ 换行、Enter 确认），全部确认后保存本件并同步标定件。</p>
    {current?.error && <Notice title="无法给出对齐建议" tone="warn">{current.error}</Notice>}
    {!current && <p className="muted">正在按图像对齐…</p>}
    {ready && <div className="alr-body" tabIndex={0} onKeyDown={onKey} aria-label="边界复核">
      {value.cuts.map((item, at) => {
        const cut = cuts[at];
        const label = `段 ${at + 1} → 段 ${at + 2} 的边界`;
        // 可信且已勾选的边界收起成一行，点开或移到这一行时再展开
        if (!item.review && checked[at] && at !== row) {
          return <section key={at} className="alr-boundary collapsed" aria-label={label} onClick={() => setRow(at)}>
            <header>
              <strong>段 {at + 1} → 段 {at + 2}</strong>
              <span className="muted">边界 {cut}（按比例 {item.proportional}）· 对齐可信</span>
              <Badge tone="ok">已确认</Badge>
            </header>
          </section>;
        }
        // 只看边界两侧段的检测相机（D-SB-41），相机少时缩略图更大
        const shown = plan.cameras.filter(camera => [plan.segments[at], plan.segments[at + 1]].some(segment => segment && segmentCameras(segment, plan.cameras).includes(camera)));
        return <section key={at} className={`alr-boundary${at === row ? " active" : ""}`} aria-label={label} onClick={() => setRow(at)}>
          <header>
            <strong>段 {at + 1} → 段 {at + 2}</strong>
            <span className="plan-mono muted">{shown.join(" · ")}</span>
            <span className="muted">建议 {item.cut}（按比例 {item.proportional}）</span>
            {item.reason && <Badge tone="warn">需重点复核：{reasonText[item.reason]}</Badge>}
            {checked[at] && <Badge tone="ok">已确认</Badge>}
          </header>
          <div className="alr-compare">
            <BoundaryFrames label="示教件" strips={templateThumbs.strips!} cameras={shown} cut={value.templateCuts[at]} />
            <BoundaryFrames label="本件" strips={roundThumbs.strips!} cameras={shown} cut={cut} />
          </div>
          <div className="wp-actions">
            <Button size="sm" aria-label={`段 ${at + 1} 边界前移`} onClick={() => nudge(at, -1)}>←</Button>
            <output aria-label={`段 ${at + 1} 边界`} className="plan-mono">{cut}</output>
            <Button size="sm" aria-label={`段 ${at + 1} 边界后移`} onClick={() => nudge(at, 1)}>→</Button>
            <Button size="sm" disabled={checked[at]} onClick={() => confirmRow(at)}>确认此边界</Button>
          </div>
        </section>;
      })}
      {locked && <p className="muted">{locked}</p>}
      <div className="wp-actions">
        <Button variant="filled" disabled={blocked} onClick={() => void save()}>确认本件边界</Button>
        <span className="muted">已确认 {checked.filter(Boolean).length} / {checked.length}</span>
      </div>
    </div>}
  </section>;
}
