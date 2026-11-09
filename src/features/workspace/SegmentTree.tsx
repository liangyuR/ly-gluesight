import { useEffect, useState, type ReactElement } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { Button, Checkbox, IconButton, Surface, Switch } from "qing-steel";
import { WithReason } from "./components";
import type { CameraParamsTask, CorridorPhase } from "./cameraTasks";
import { setSkips, valid, type PassNode } from "./workbenchDraft";
import type { SegmentDraft } from "./useSegmentDraft";

type Dot = "ok" | "warn" | "off" | "skip";
const kindText = { straight: "直线", corner: "弯道" } as const;

/** 一组走廊的完成状态：全部已示教为 ok，有进展为 warn，都没开始为 off，没有要示教的为 skip。 */
function dotOf(phases: CorridorPhase[]): Dot {
  if (!phases.length) return "skip";
  if (phases.every(p => p === "done")) return "ok";
  return phases.some(p => p !== "selectFrame") ? "warn" : "off";
}

/**
 * 分段与示教页左栏：按笔列出段，每段下列出检测相机的走廊状态；可多选笔批量设为不检测或恢复检测，可隐藏不检测的笔。
 * 底部是相机参数（像素当量、出胶点等按相机设置）。locked 非空时不能切到别的段或相机（原因同时作为提示）。
 */
export default function SegmentTree({ seg, tree, currentPos, camera, paramsCamera, corridors, cameraItems, locked, onSegment, onCorridor, onParams }: {
  seg: SegmentDraft; tree: PassNode[]; currentPos: number | null; camera: string; paramsCamera: string | null;
  /** 段位置 → 各检测相机的走廊状态；段结构有未保存的改动时为 null */
  corridors: (pos: number) => { camera: string; status: CorridorPhase }[] | null;
  cameraItems: CameraParamsTask[]; locked: string;
  onSegment: (pos: number) => void; onCorridor: (pos: number, camera: string) => void; onParams: (camera: string) => void;
}) {
  const currentPass = tree.find(node => node.segments.some(item => item.pos === currentPos))?.pass ?? null;
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(() => new Set(currentPass ? [currentPass] : []));
  useEffect(() => { if (currentPass) setExpanded(open => open.has(currentPass) ? open : new Set([...open, currentPass])); }, [currentPass]);
  const [hideSkipped, setHideSkipped] = useState(false);
  const [picked, setPicked] = useState<ReadonlySet<number>>(new Set());
  const [batchError, setBatchError] = useState("");
  const allSkipped = (node: PassNode) => node.segments.every(item => item.skip);
  const shown = tree.filter(node => !hideSkipped || !allSkipped(node) || node.pass === currentPass);
  const phasesOf = (pos: number) => (corridors(pos) ?? []).map(item => item.status);
  const done = tree.flatMap(node => node.segments.flatMap(item => phasesOf(item.pos))).filter(p => p === "done").length;
  const total = tree.flatMap(node => node.segments.flatMap(item => phasesOf(item.pos))).length;

  const guard = (current: boolean, button: ReactElement<{ "aria-describedby"?: string }>, key: string) =>
    locked && !current ? <WithReason key={key} block reason={locked}>{button}</WithReason> : <span key={key} className="wp-group-slot">{button}</span>;
  const batch = (skip: boolean) => {
    if (!seg.draft) return;
    const positions = tree.filter(node => picked.has(node.pass)).flatMap(node => node.segments.map(item => item.pos));
    const next = setSkips(seg.draft, positions, skip);
    if (!valid(next)) { setBatchError("至少要有一段检测"); return; }
    setBatchError(""); seg.change(next); setPicked(new Set());
  };
  const togglePick = (pass: number, on: boolean) => setPicked(current => {
    const next = new Set(current);
    if (on) next.add(pass); else next.delete(pass);
    return next;
  });

  return <Surface role="region" className="wp-group-panel st-tree" aria-label="笔与段">
    <h2>笔与段 <small>走廊 {done} / {total}</small></h2>
    <Switch label="隐藏不检测的笔" checked={hideSkipped} onCheckedChange={setHideSkipped} />
    {picked.size > 0 && <div className="st-batch" role="group" aria-label="批量设置所选的笔">
      <span>已选 {picked.size} 笔</span>
      <Button size="sm" disabled={!!seg.locked} onClick={() => batch(true)}>设为不检测</Button>
      <Button size="sm" disabled={!!seg.locked} onClick={() => batch(false)}>恢复检测</Button>
      <Button size="sm" variant="text" onClick={() => { setPicked(new Set()); setBatchError(""); }}>取消</Button>
      {batchError && <span className="c-warn" role="alert">{batchError}</span>}
    </div>}
    <ul className="st-passes" aria-label="笔">
      {shown.map(node => {
        const open = expanded.has(node.pass);
        const passDot = dotOf(node.segments.flatMap(item => phasesOf(item.pos)));
        const isCurrent = node.pass === currentPass && paramsCamera == null;
        const first = node.segments[0];
        return <li key={node.pass} className={"st-pass" + (node.short || allSkipped(node) ? " dim" : "")}>
          <div className="st-pass-row">
            <Checkbox label={<span className="qs-sr-only">选择第 {node.pass} 笔</span>} checked={picked.has(node.pass)} disabled={!!seg.locked} onCheckedChange={on => togglePick(node.pass, on)} />
            {guard(isCurrent, <Button variant={isCurrent ? "tonal" : "text"} size="sm" className="wp-group-item st-pass-btn" aria-label={`第 ${node.pass} 笔，${node.last - node.first + 1} 帧`} disabled={!!locked && !isCurrent}
              onClick={() => { if (first) onSegment(first.pos); }}>
              <i className={"dot " + passDot} aria-hidden="true" /><strong>第 {node.pass} 笔</strong><span>{node.last - node.first + 1} 帧{node.short ? " · 短笔" : ""}</span>
            </Button>, "pass")}
            <IconButton size="sm" aria-label={`${open ? "收起" : "展开"}第 ${node.pass} 笔`} aria-expanded={open}
              onClick={() => setExpanded(current => { const next = new Set(current); if (!next.delete(node.pass)) next.add(node.pass); return next; })}>
              {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
            </IconButton>
          </div>
          {open && <ul className="st-segments" aria-label={`第 ${node.pass} 笔的段`}>{node.segments.map(item => {
            const list = corridors(item.pos);
            const selected = item.pos === currentPos && paramsCamera == null;
            return <li key={item.pos} className={"st-seg" + (item.skip ? " dim" : "")}>
              {guard(selected, <Button variant={selected ? "tonal" : "text"} size="sm" className="wp-group-item st-seg-btn" aria-label={`选择 ${item.label} 段`} aria-pressed={selected} disabled={!!locked && !selected}
                onClick={() => onSegment(item.pos)}>
                <i className={"dot " + (item.skip ? "skip" : dotOf((list ?? []).map(c => c.status)))} aria-hidden="true" /><strong>{item.label}</strong><em>{item.skip ? "不检测" : kindText[item.kind]}</em><span>{item.last - item.first + 1} 组</span>
              </Button>, "seg")}
              {!item.skip && list && list.length > 0 && <span className="st-cams">{list.map(c => {
                const on = selected && c.camera === camera;
                return guard(on, <Button variant={on ? "tonal" : "text"} size="sm" className="st-cam" aria-label={`选择 ${c.camera} ${item.label} 走廊`} aria-pressed={on} disabled={!!locked && !on}
                  title={`${item.label} · ${c.camera} · ${c.status === "done" ? "已示教" : c.status === "teach" ? "待画走廊" : "待选帧"}`} onClick={() => onCorridor(item.pos, c.camera)}>
                  <i className={"dot " + dotOf([c.status])} aria-hidden="true" />{c.camera}
                </Button>, c.camera);
              })}</span>}
            </li>;
          })}</ul>}
        </li>;
      })}
    </ul>
    <h2>相机参数</h2>
    <nav className="wp-group-cameras" aria-label="相机参数选择">{cameraItems.map(item => guard(item.camera === paramsCamera,
      <Button variant={item.camera === paramsCamera ? "tonal" : "text"} size="sm" className={"wp-group-item wp-group-camera" + (item.camera === paramsCamera ? " selected" : "")} aria-label={"选择相机 " + item.camera} aria-pressed={item.camera === paramsCamera}
        disabled={!!locked && item.camera !== paramsCamera} title={item.mmPerPx != null ? `${item.camera} · ${Number(item.mmPerPx.toPrecision(6))} mm/px` : `${item.camera} · 缺像素当量`} onClick={() => onParams(item.camera)}>
        <i className={"dot " + (item.mmPerPx != null ? "ok" : "warn")} aria-hidden="true" /><strong>{item.camera}</strong>
      </Button>, item.camera))}</nav>
  </Surface>;
}
