import { useRef, type PointerEvent } from "react";
import { Link } from "react-router-dom";
import type { PointVis, Recipe, RecipeDoc } from "../cycle/types";
import { CameraSegmentOverview } from "../plan/CameraSegmentOverview";
import type { WorkspacePlanView } from "../plan/types";
import { expectedText } from "../plan/vis";
import { shotState, shotStateLabel, visColor } from "../cycle/vis";
import { useWorkspace } from "./context";
import { Badge, GrayViewer, Notice, Panel, useGrayImage, WorkspaceBar, WorkspaceEmpty } from "./components";
import { cameraTeachingTasks, type CameraTeachingTask } from "./cameraTasks";
import type { Overview } from "./types";
import "./corridor.css";

function overviewGroups(layout: Pick<Recipe, "shots">) {
  const groups = new Map<string, number[]>();
  layout.shots.forEach((shot, k) => { const members = groups.get(shot.poseId) ?? []; members.push(k); groups.set(shot.poseId, members); });
  return [...groups.values()].map(members => {
    const enabled = members.filter(k => !layout.shots[k].skip);
    return { members, k: enabled.length === 1 ? enabled[0] : members[0], selected: enabled.length === 1 };
  });
}

export function defaultPositions(layout: Pick<Recipe, "shots">): [number,number][] {
  const groups = overviewGroups(layout);
  const n = groups.length;
  const cols = Math.max(1, Math.min(4, n, Math.ceil(Math.sqrt(n * 520 / 230))));
  const rows = Math.max(1, Math.ceil(n / cols));
  const positions: [number, number][] = new Array(layout.shots.length);
  groups.forEach(({members}, i) => members.forEach(k => { positions[k] = [(i % cols + .5) / cols, (Math.floor(i / cols) + .5) / rows]; }));
  return positions;
}
export function WorkpieceOverview({layout,overview,selected=0,onSelect,onMove,vis}:{
  layout:Recipe; overview:Overview|null; selected?:number; onSelect?:(k:number)=>void;
  onMove?:(k:number,position:[number,number])=>void; vis?:PointVis[];
}) {
  const svg = useRef<SVGSVGElement>(null);
  const drag = useRef<{k:number;start:[number,number];position:[number,number]} | null>(null);
  const positions = overview?.positions.length === layout.shots.length ? overview.positions : defaultPositions(layout);
  const point = (e:PointerEvent<SVGGElement>):[number,number] | null => {
    const matrix = svg.current?.getScreenCTM();
    if (!matrix) return null;
    const p = new DOMPoint(e.clientX,e.clientY).matrixTransform(matrix.inverse());
    return [p.x,p.y];
  };
  return <svg ref={svg} className={"wp-overview " + (onMove ? "editable" : "")} viewBox="0 0 800 420" aria-label="工件总览，选择帧查看原图">
    <rect width={800} height={420} fill="var(--surface-lowest)" />
    {overview?.background && <image href={overview.background} x={110} y={46} width={580} height={292} preserveAspectRatio="xMidYMid meet" opacity={.85} />}
    {overviewGroups(layout).map(({k, members, selected: hasSelection}) => {
      const [x,y] = positions[k];
      const shot = layout.shots[k];
      const state = vis ? shotState(layout, vis, k) : null;
      const tone = state && state !== "skip" && state !== "untaught" && state !== "none" ? visColor[state] : null;
      const on = members.includes(selected);
      return <g key={k} role="button" tabIndex={0} aria-label={"总览选择帧 k" + (k+1)} aria-pressed={on} data-state={state ?? undefined}
      onPointerDown={e => {onSelect?.(k);const p=point(e);if(onMove&&p){e.currentTarget.setPointerCapture(e.pointerId);drag.current={k,start:p,position:[x,y]};}}} onPointerMove={e => {
        const p=point(e),d=drag.current;if(!p||!d||!onMove)return;
        onMove(d.k,[Math.min(1,Math.max(0,d.position[0]+(p[0]-d.start[0])/520)),Math.min(1,Math.max(0,d.position[1]+(p[1]-d.start[1])/230))]);
      }} onPointerUp={() => {drag.current=null;}} onPointerCancel={() => {drag.current=null;}} onClick={() => onSelect?.(k)} onKeyDown={e => {
        if(e.key==="Enter"||e.key===" "){e.preventDefault();onSelect?.(k);}
        if(onMove&&e.key.startsWith("Arrow")){e.preventDefault();const step=e.shiftKey ? 0.04 : 0.01;onMove(k,[Math.min(1,Math.max(0,x+(e.key==="ArrowRight"?step:e.key==="ArrowLeft"?-step:0))),Math.min(1,Math.max(0,y+(e.key==="ArrowDown"?step:e.key==="ArrowUp"?-step:0)))]);}
      }}>
      <rect x={140+x*520-57} y={74+y*230-38} width={114} height={76} rx={10} fill={on ? "var(--primary-container)" : "var(--surface-low)"} fillOpacity={.84} stroke={on?"var(--primary)":tone ?? "var(--outline)"} strokeWidth={on||tone?2:1} strokeDasharray={shot?.skip||state==="untaught"?"5 4":undefined} />
      <text x={140+x*520} y={74+y*230-6} textAnchor="middle" fill={on?"var(--primary)":"var(--on-surface-variant)"} fontSize={15}>{shot.poseId}</text>
      <text x={140+x*520} y={74+y*230+14} textAnchor="middle" fill="var(--on-surface-variant)" fontSize={11} className="mono">{hasSelection ? `${shot.camera} · 视角 ${shot.view}` : "待选检测图"}</text>
      {state && <text x={140+x*520} y={74+y*230+30} textAnchor="middle" fill={tone ?? "var(--on-surface-variant)"} fontSize={11}>{shotStateLabel[state]}</text>}
    </g>;})}
    <text x={400} y={373} textAnchor="middle" fill="var(--on-surface-variant)" fontSize={12}>{onMove ? "拖动帧框或用方向键微调显示位置；拍照与测量不受影响" : "显示布置与生产配方快照绑定"}</text>
  </svg>;
}

function CameraFrame({ docId, task, corridor }: { docId: string; task: CameraTeachingTask; corridor: [number, number][] }) {
  const { image, error, loading } = useGrayImage(docId, task.frame?.id ?? null);
  return <>
    <div className="wp-actions"><strong>{task.camera}</strong><Badge tone={task.status === "done" ? "ok" : task.frame ? "warn" : "neutral"}>{task.label}</Badge><Link className="wp-text-link" to={task.href}>{task.frame ? "查看走廊" : "去选示教帧"}</Link></div>
    <GrayViewer image={image} loading={loading} error={error} label={task.frame ? `${task.camera} · 示教帧` : `${task.camera} · 尚未选示教帧`} overlay={corridor.length >= 2 ? { path: corridor } : undefined}
      emptyTitle="尚未选示教帧" emptyHint="在“相机走廊示教”里为这台相机从采集轮次选一帧。"/>
  </>;
}

/** 段配方的总览：三台相机与沿胶路的段条（N_exp 或“待标定”），以及每台相机的示教帧和走廊。 */
function SegmentOverview({ plan, doc }: { plan: WorkspacePlanView; doc: RecipeDoc }) {
  const tasks = cameraTeachingTasks(plan);
  return <div className="wp-page"><WorkspaceBar />
    <Panel title="相机与段" detail="相机由同一路高电平启停，各自按设定帧率采集；段沿胶路顺序排列，预期帧数由多轮标定得出">
      <CameraSegmentOverview cameras={plan.cameras} segments={plan.segments} />
      <div className="wp-actions" style={{ marginTop: 12 }}>
        <Badge tone={plan.calibrated ? "ok" : "warn"}>{plan.calibrated ? `预期总数 ${expectedText(plan.expectedTotal, plan.toleranceTotal)}` : "预期总数待标定"}</Badge>
        <Link className="wp-text-link" to="/recipe/segments">段列表</Link><Link className="wp-text-link" to="/recipe/calibration">多轮标定</Link>
      </div>
    </Panel>
    <Panel title="示教帧与走廊" detail="每台相机一条走廊，按直线段几何，所有帧共用">
      <ul className="wp-segment-teach" aria-label="各相机示教帧">{tasks.map(task => <li key={task.camera}><CameraFrame docId={doc.id} task={task} corridor={doc.inspect?.find(item => item.camera === task.camera)?.corridor ?? []} /></li>)}</ul>
    </Panel>
    <Notice title="总览只读">段配方的总览由计划自动给出，不再摆放拍照点框。</Notice>
  </div>;
}

export default function OverviewPage() {
  const { data, doc } = useWorkspace();
  return data?.plan && doc ? <SegmentOverview plan={data.plan} doc={doc} /> : <WorkspaceEmpty />;
}
