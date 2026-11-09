import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { Button, IconButton } from "qing-steel";
import type { PassSkip, PathSegment } from "../plan/types";
import { segmentLabel } from "../plan/vis";
import { useHorizontalWindow } from "../../lib/useHorizontalWindow";
import { Notice } from "./components";
import SegmentCoveragePreview from "./SegmentCoveragePreview";
import { ignoreKey, Thumb, useThumbs } from "./thumbs";
import { moveCut, passCells, toggleCamera, toggleCut, toggleKind, type PassNode } from "./workbenchDraft";
import type { SegmentDraft } from "./useSegmentDraft";

const CELL = 48, GAP = 2;
const passSkipText = { head: "开头跳过", tail: "结尾跳过", short: "太短，整笔不检" } as const;

/**
 * 当前笔的帧条（分段与示教页中栏上方）：只显示这一笔的帧组。段名带点击按 直线 → 弯道 → 不检测 循环，段下点相机选检测相机，
 * 帧间竖缝点击插入或删除边界、拖动已有边界移动它；笔末边界固定。点缩略图选帧组（即当前段的候选示教帧），B 插删边界，← → 换帧组。
 */
export default function PassStrip({ seg, cameras, node, passCount, segments, passes, passSkip, seq, teachSeq, camera, onSeq }: {
  seg: SegmentDraft; cameras: string[]; node: PassNode; passCount: number;
  /** 已保存的段列表：段结构没改时按它标段首尾跳过 */
  segments: PathSegment[];
  passes: number[] | undefined; passSkip: PassSkip | undefined;
  seq: number; teachSeq: number | null; camera: string; onSeq: (seq: number) => void;
}) {
  const draft = seg.draft!;
  const locked = seg.locked;
  const count = node.last - node.first + 1;
  const local = Math.min(count - 1, Math.max(0, seq - node.first));
  const timeline = useHorizontalWindow<HTMLDivElement>(count, CELL + GAP, local, 58);
  const thumbs = useThumbs(seg.roundId, cameras, seg.groupCount, node.first - 1 + timeline.start, node.first - 1 + timeline.end);
  const visible = Array.from({ length: timeline.end - timeline.start }, (_, i) => node.first + timeline.start + i);
  const cells = passCells(passes, seg.groupCount, passSkip);
  const keyboardSelection = useRef<string | null>(null);
  const [dragging, setDragging] = useState<number | null>(null);
  useEffect(() => {
    if (dragging == null) return;
    const end = () => setDragging(null);
    window.addEventListener("pointerup", end);
    return () => window.removeEventListener("pointerup", end);
  }, [dragging]);
  useEffect(() => {
    if (!keyboardSelection.current) return;
    const cell = Array.from(timeline.ref.current?.querySelectorAll<HTMLButtonElement>(".swb-cell.selected") ?? []).find(item => item.dataset.camera === keyboardSelection.current);
    if (cell) { cell.focus({ preventScroll: true }); keyboardSelection.current = null; }
  }, [seq, timeline.start, timeline.end]);

  const change = seg.change;
  const cut = (after: number) => change(toggleCut(draft, after, seg.ends));
  const onKey = (e: KeyboardEvent) => {
    if (ignoreKey(e)) return;
    if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) {
      e.preventDefault();
      keyboardSelection.current = (e.target as HTMLElement).closest(".swb-cell")?.getAttribute("data-camera") ?? cameras[0];
      onSeq(e.key === "Home" ? node.first : e.key === "End" ? node.last : Math.min(seg.groupCount, Math.max(1, seq + (e.key === "ArrowLeft" ? -1 : 1))));
    } else if ((e.key === "b" || e.key === "B") && !locked && seq >= node.first && seq < node.last) {
      e.preventDefault();
      cut(seq);
    }
  };
  const columns = `56px repeat(${count}, var(--swb-cell))`;
  const col = (group: number) => group - node.first + 2;
  const at = (group: number) => node.segments.find(item => group >= item.first && group <= item.last);
  const lockTitle = locked || undefined;
  const params = (pos: number) => seg.restructured ? undefined : segments[pos];

  return <section className="swb-body ps-strip" aria-label={`第 ${node.pass} 笔帧条`} onKeyDown={onKey}>
    <div className="swb-toolbar">
      <div className="swb-summary" role="status" aria-label="分段草稿">
        <IconButton size="sm" aria-label="上一笔" disabled={node.pass <= 1} onClick={() => onSeq(node.first - 1)}><ChevronLeft size={15} /></IconButton>
        <strong>第 {node.pass} / {passCount} 笔</strong>
        <IconButton size="sm" aria-label="下一笔" disabled={node.pass >= passCount} onClick={() => onSeq(node.last + 1)}><ChevronRight size={15} /></IconButton>
        <span>帧组 {node.first}–{node.last} · {count} 组 · {node.segments.length} 段</span>
        {node.short && <span className="muted">（短笔：整笔不检）</span>}
      </div>
      <div className="wp-actions">
        {locked && <span className="muted">{locked}</span>}
        {seq >= node.first && seq < node.last && <Button size="sm" disabled={!!locked} onClick={() => cut(seq)}>
          {draft.cuts.includes(seq) ? `删除帧组 ${seq} 后的边界` : `在帧组 ${seq} 后插入边界`}
        </Button>}
        {seg.edited && <Button size="sm" disabled={seg.status === "saving"} onClick={seg.undo}>撤销改动</Button>}
      </div>
    </div>
    {seg.impact && <Notice title="这次分段会清掉已示教的走廊" tone="warn">
      {seg.impact.dropped.join("、")} 的示教帧会落到别的段或段类型变了，保存后这些走廊要重示。
      <span className="wp-actions"><Button size="sm" variant="filled" disabled={!!locked} onClick={seg.confirm}>确认保存</Button>{seg.edited && <Button size="sm" onClick={seg.undo}>撤销</Button>}</span>
    </Notice>}
    {!!seg.coverage?.zeroCoverageSegments.length && <Notice title="有检测段没有有效帧" tone="warn">
      第 {seg.coverage.zeroCoverageSegments.join("、")} 段在边界保护或跳过范围展开后应测帧数为 0，请复核边界与跳过设置。
      <details><summary>检测覆盖</summary><SegmentCoveragePreview coverage={seg.coverage} /></details>
    </Notice>}
    <div className="swb-scroll" ref={timeline.ref} onScroll={timeline.onScroll}>
      <div className="swb-grid" style={{ gridTemplateColumns: columns }}>
        <span className="swb-label" style={{ gridRow: 1, gridColumn: 1 }}>段</span>
        {node.segments.map(item => {
          const label = segmentLabel({ kind: item.kind, skip: item.skip });
          return <Button variant="text" size="sm" key={`${item.first}-${item.kind}-${item.skip}`} type="button" className={`swb-seg ${item.skip ? "skip" : item.kind}`} style={{ gridRow: 1, gridColumn: `${col(item.first)} / ${col(item.last) + 1}` }}
            aria-label={`${item.label}（${label}，帧组 ${item.first}–${item.last}），点击切换类型`} title={lockTitle} disabled={!!locked} onClick={() => change(toggleKind(draft, item.pos))}>
            {item.label} · {label} · {item.last - item.first + 1} 组
          </Button>;
        })}
        <span className="swb-label" style={{ gridRow: 2, gridColumn: 1 }} title="点相机名决定该段由哪几台相机检测（至少一台）；不检测的段不用相机">检测相机</span>
        {node.segments.map(item => <div key={`cams-${item.first}`} className="swb-cams" style={{ gridRow: 2, gridColumn: `${col(item.first)} / ${col(item.last) + 1}` }}
          role="group" aria-label={`${item.label} 检测相机`}>
          {cameras.map(id => {
            const on = !item.skip && item.cameras.includes(id);
            return <Button variant="text" size="sm" key={id} type="button" className={`swb-cam${on ? " on" : ""}`} aria-pressed={on} disabled={!!locked || item.skip}
              title={item.skip ? `${item.label} 不检测：点段名切回直线后可选相机` : on ? `${item.label} 由 ${id} 检测，点击取消` : `点击让 ${id} 检测 ${item.label}`}
              onClick={() => change(toggleCamera(draft, item.pos, id, cameras))}>{id}</Button>;
          })}
        </div>)}
        {cameras.map((id, row) => [
          <span key={`${id}-label`} className="swb-label" style={{ gridRow: row + 3, gridColumn: 1 }}>{id}</span>,
          ...visible.map(group => {
            const item = at(group);
            const used = !!item && !item.skip && item.cameras.includes(id);
            const segment = item && params(item.pos);
            const segmentSkipped = !!item && !!segment && (group < item.first + segment.skipHead || group > item.last - segment.skipTail);
            const pass = cells?.[group - 1];
            const skipped = segmentSkipped || !!pass?.skip;
            const title = segmentSkipped ? `帧组 ${group} 在 ${item!.label} 的跳过范围内，不判定`
              : pass?.skip ? `帧组 ${group}：第 ${pass.pass} 笔${passSkipText[pass.skip]}，不判定` : `帧组 ${group}${item ? ` · ${item.label}` : ""}`;
            const teach = id === camera && group === teachSeq;
            return <Button variant="text" size="sm" key={`${id}-${group}`} data-camera={id} type="button" aria-label={`${id} 帧组 ${group}`} aria-pressed={group === seq} style={{ gridRow: row + 3, gridColumn: col(group) }}
              className={`swb-cell${group === seq ? " selected" : ""}${draft.cuts.includes(group) ? " cut" : ""}${used ? "" : " unused"}${skipped ? " skipped" : ""}${teach ? " teach" : ""}`}
              onClick={() => onSeq(group)} title={teach ? `${title} · ${id} 的示教帧` : title}>
              <Thumb {...thumbs.getFrame(id, group - 1)} />
            </Button>;
          })])}
        <span className="swb-label" style={{ gridRow: cameras.length + 3, gridColumn: 1 }} title="点 ＋ 在该帧组之后插入边界，点 × 删除；按住 × 拖到别的竖缝移动边界">边界</span>
        {visible.map(group => {
          const position = { gridRow: cameras.length + 3, gridColumn: col(group) };
          if (group === node.last) return <span key="end" className="swb-seam-end" style={position} title={node.pass < passCount ? "笔末：段不跨笔，这里固定是边界" : undefined}>{group}</span>;
          const index = draft.cuts.indexOf(group);
          return <Button variant="text" size="sm" key={`seam-${group}`} style={position} type="button" className={`swb-seam${index >= 0 ? " on" : ""}${dragging != null && dragging !== index ? " target" : ""}`} disabled={!!locked}
            aria-label={index >= 0 ? `删除帧组 ${group} 之后的边界` : `在帧组 ${group} 之后插入边界`}
            onPointerDown={() => { if (index >= 0) setDragging(index); }}
            onPointerUp={() => { if (dragging != null && draft.cuts[dragging] !== group) change(moveCut(draft, dragging, group, seg.ends)); }}
            onClick={() => cut(group)}><span>{group}</span><b aria-hidden="true">{index >= 0 ? "×" : "＋"}</b></Button>;
        })}
      </div>
    </div>
    {thumbs.error && <Notice title="读取示教件图像失败" tone="warn">{thumbs.error}<Button size="sm" onClick={thumbs.retry}>重试缩略图</Button></Notice>}
  </section>;
}
