import { useEffect, useRef, useState, type MutableRefObject } from "react";
import { Link } from "react-router-dom";
import { Button, NumberField, Select, Switch } from "qing-steel";
import { CameraSegmentOverview } from "../plan/CameraSegmentOverview";
import { KindChecksEditor } from "../plan/KindChecksEditor";
import type { OffsetMeasure, PathSegment, SegmentChecks, WorkspacePlanView } from "../plan/types";
import type { WorkspaceView } from "./types";
import { expectedText, PLAN_CAMERAS, segmentKindLabel } from "../plan/vis";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, Notice, Panel, WorkspaceBar, WorkspaceEmpty } from "./components";
import { errorText } from "../../lib/errors";
import { patchSegment, segmentIssues } from "./segmentEdit";
import SegmentWorkbench from "./SegmentWorkbench";
import { segmentCameras } from "./workbenchDraft";
import AlignReview from "./AlignReview";
import "./segments.css";

const DEBOUNCE_MS = 700;
type SaveState = "saved" | "pending" | "saving" | "failed";
const saveText: Record<SaveState, string> = { saved: "段列表已保存", pending: "等待保存…", saving: "保存中…", failed: "保存失败" };
const measureOptions: { value: OffsetMeasure; label: string }[] = [
  { value: "absoluteX", label: "图像横向坐标（绝对 x）" },
  { value: "toEdge", label: "胶条到边缘距离" },
];

/**
 * 段列表与段检测项（段配方，路由 /recipe/segments）：顶部分段标定工作台（SegmentWorkbench）按图像定段结构与模板轮边界；
 * 下方设各段长度、帧数容差、检测区间与检测项。预期帧数 N_exp 与标定统计只读：只能由多轮采集得出（多轮标定页）。
 */
export default function SegmentsPage() {
  const { data, doc, selectedId } = useWorkspace();
  // 编辑器卸载时据此判断：只是离开本页（仍是这个配方），还是换了配方（见 SegmentsEditor 的卸载提交）。页面本身在换配方时不卸载，先于编辑器的清理更新
  const selected = useRef(selectedId);
  selected.current = selectedId;
  if (!data || !doc) return <WorkspaceEmpty />;
  const plan = data.plan;
  if (!plan) return <Notice title="该草稿不是段配方" tone="warn">请新建段配方后编辑段列表，旧拍照点草稿不再支持。</Notice>;
  return <SegmentsEditor key={doc.id} plan={plan} selected={selected} />;
}

function SegmentsEditor({ plan, selected }: { plan: WorkspacePlanView; selected: MutableRefObject<string | null> }) {
  const { data, doc, act, busy, setError } = useWorkspace();
  // 未保存的本地编辑；null 时显示后端给的 plan.segments
  const [edits, setEdits] = useState<PathSegment[] | null>(null);
  const [state, setState] = useState<SaveState>("saved");
  const [retry, setRetry] = useState(0);
  const [workbenchDirty, setWorkbenchDirty] = useState(false);
  const immediate = useRef(false);
  const failed = useRef("");
  const saved = useRef("");
  // 正在提交的那一份（防抖到期后到保存完成之间），卸载时据此避免重复提交、并在它完成后接着提交更新的编辑
  const saving = useRef<{ signature: string; request: Promise<WorkspaceView | null> } | null>(null);
  const segments = edits ?? plan.segments;
  const issues = edits ? segmentIssues(edits) : [];
  const blocked = issues.length > 0;
  // 有未保存编辑时，保存状态至少是“等待保存”（effect 还没来得及把 state 置为 pending 的那一帧也如此）
  const shown: SaveState = edits ? (state === "saved" ? "pending" : state) : "saved";
  const frozen = !!data?.workspace.pending;
  const docId = doc?.id;
  const revision = data?.workspace.revision;

  useEffect(() => {
    if (!edits || blocked || busy || frozen || docId == null || revision == null) return;
    const signature = JSON.stringify(edits);
    if (failed.current === signature || saved.current === signature) return;
    setState("pending");
    const timer = setTimeout(async () => {
      setState("saving");
      const request = act(() => workspaceApi.updateSegments(docId, revision, edits), "段列表已保存");
      saving.current = { signature, request };
      const result = await request;
      if (saving.current?.request === request) saving.current = null;
      if (result) { saved.current = signature; failed.current = ""; setEdits(current => current === edits ? null : current); setState("saved"); }
      else { failed.current = signature; setState("failed"); }
    }, immediate.current ? 0 : DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [edits, blocked, busy, frozen, docId, revision, retry, act]);

  // 离开本页时，防抖还没到期（或上一份还在提交）的编辑立即提交，不因卸载丢掉；校验不过、被冻结、已保存或已失败的不提交
  const latest = useRef({ edits, blocked, frozen, docId, revision, act, setError });
  latest.current = { edits, blocked, frozen, docId, revision, act, setError };
  useEffect(() => () => {
    const { edits: unsaved, blocked: invalid, frozen: locked, docId: id, revision: current, act: run, setError: report } = latest.current;
    if (!unsaved || invalid || locked || id == null || current == null) return;
    const signature = JSON.stringify(unsaved);
    if (saved.current === signature || failed.current === signature || saving.current?.signature === signature) return;
    const submit = (revision: number) => {
      // 仍是这个配方：经 act 提交，工作区拿到新修订。已经换了配方：act 会打断 select 的加载，只能直接提交到原配方
      if (selected.current === id) void run(() => workspaceApi.updateSegments(id, revision, unsaved), "段列表已保存");
      else void workspaceApi.updateSegments(id, revision, unsaved).catch(e => report(`配方 ${id} 的段列表未能保存：${errorText(e)}`));
    };
    // 上一份还在提交：等它完成，用它返回的新修订接着提交（act 同一时刻只做一件事）
    if (saving.current) void saving.current.request.then(next => { if (next) submit(next.workspace.revision); });
    else submit(current);
  }, []);

  const edit = (next: PathSegment[]) => { immediate.current = false; setEdits(next); };
  const patch = (at: number, change: Partial<PathSegment>) => edit(patchSegment(segments, at, change));
  // 工作台与参数表都会写段列表：一边有未保存的改动时另一边不能提交，避免修订号冲突
  const paramsPending = edits ? "段参数还没保存完，保存后再保存分段" : "";
  const cardsLocked = frozen || workbenchDirty;
  const pending = segments.filter(segment => segment.expectedFrames == null).map(segment => segment.index);

  return <div className="wp-page"><WorkspaceBar />
    <SegmentWorkbench plan={plan} locked={paramsPending} onDirty={setWorkbenchDirty} />
    <AlignReview plan={plan} locked={workbenchDirty ? "上方分段有未保存的改动，先保存或撤销后再确认其余轮次" : paramsPending} />
    <Panel title="段列表" detail="各段帧数容差、长度、检测区间与检测项；段的数量、顺序与类型在上方分段标定里按图像划分，预期帧数由多轮采集得出，这里只读">
      <CameraSegmentOverview cameras={plan.cameras} segments={segments} />
      {frozen && <Notice title="待生效，候选已冻结">等待当前工件结束后切换；生效或放弃后再修改段列表。</Notice>}
      {workbenchDirty && <Notice title="分段有未保存的改动">先保存或撤销上方的分段改动，再编辑段参数。</Notice>}
      <div className="seg-savebar" role="status" aria-label="段列表保存状态">
        {blocked ? <span className="warn">{issues.join("；")}（修正后自动保存）</span> : <span className={shown === "failed" ? "warn" : undefined}>{saveText[shown]}</span>}
        {shown === "failed" && !blocked && <Button size="sm" onClick={() => { failed.current = ""; setRetry(n => n + 1); }}>重试保存</Button>}
      </div>
      <ol className="seg-list" aria-label="段">
        {segments.map((segment, at) => <SegmentCard key={at} segment={segment} cameras={plan.cameras} disabled={cardsLocked}
          onPatch={change => patch(at, change)} />)}
      </ol>
    </Panel>
    <Panel title="整件预期帧数" detail="各段预期帧数之和；各段都待标定时取整件多轮采集的帧组数均值">
      <div className="seg-total">
        <strong aria-label="整件预期总帧数">{expectedText(plan.expectedTotal, plan.toleranceTotal)}</strong>
        <span className="muted">ΣN_exp ± Σtol{plan.cameras.length !== PLAN_CAMERAS && `（需要 ${PLAN_CAMERAS} 台相机，当前 ${plan.cameras.length} 台）`}</span>
        {plan.expectedTotal == null
          ? <Badge tone="warn">待标定{pending.length > 0 && "：" + pending.map(index => `段 ${index}`).join("、")}</Badge>
          : pending.length > 0 && <Badge tone="neutral">{pending.map(index => `段 ${index}`).join("、")} 待逐段标定，运行时按整件核对帧数</Badge>}
        <Link className="wp-progress-link" to="/recipe/calibration">前往多轮标定</Link>
      </div>
      <p className="muted">按段类型的默认检测项与帧数容差（默认 ±1）在“默认参数”页设置，新建段时取用。</p>
    </Panel>
  </div>;
}

interface CardProps {
  segment: PathSegment; cameras: string[]; disabled: boolean; onPatch: (change: Partial<PathSegment>) => void;
}

function SegmentCard({ segment, cameras, disabled, onPatch }: CardProps) {
  const name = `段 ${segment.index}`;
  const straight = segment.kind === "straight";
  const range = segment.inspectRange;
  const offsetRef = segment.offsetRef;
  const measure = offsetRef?.measure ?? "absoluteX";
  // 胶偏参考相机只能是本段的检测相机（D-SB-41），检测相机在上方分段标定里选
  const inspecting = segmentCameras(segment, cameras);
  const camera = offsetRef?.camera ?? inspecting[0];
  const setChecks = (checks: SegmentChecks) => onPatch({ checks });
  // 改胶偏参考的测量量或相机：旧 x_ref 不再适用，清空等标准件标定重算
  const setReference = (next: { measure?: OffsetMeasure; camera?: string }) => onPatch({ offsetRef: { measure, camera, ...next, xRef: null, spread: null } });
  return <li><section className="seg-card" aria-label={name}>
    <div className="seg-card-head">
      <strong>{name} · {segmentKindLabel[segment.kind]}</strong>
      <Badge tone={segment.expectedFrames == null ? "warn" : "ok"}>{segment.expectedFrames == null ? "待标定" : `预期 ${segment.expectedFrames} ± ${segment.tolerance} 帧`}</Badge>
      <span className="muted plan-mono" aria-label={`${name} 检测相机`}>检测相机 {inspecting.join(" · ")}</span>
    </div>
    <div className="seg-fields">
      <div className="seg-readonly">
        <span>预期帧数 N_exp（只读）</span>
        <output aria-label={`${name} 预期帧数`} className={segment.expectedFrames == null ? "seg-pending" : undefined}>{segment.expectedFrames == null ? "待标定" : segment.expectedFrames}</output>
        {segment.calib && <small>{segment.calib.rounds} 轮 · {segment.calib.min}–{segment.calib.max} · 均值 {segment.calib.mean}</small>}
      </div>
      <NumberField label="帧数容差" unit="帧" aria-label={`${name} 帧数容差`} step={1} min={0} disabled={disabled}
        value={Number.isFinite(segment.tolerance) ? segment.tolerance : ""} onValueChange={value => onPatch({ tolerance: value === "" ? NaN : value })} />
      <NumberField label="长度" unit="mm" aria-label={`${name} 长度`} step={1} min={0} disabled={disabled}
        value={segment.lengthMm ?? ""} onValueChange={value => onPatch({ lengthMm: value === "" ? null : value })} />
      <Switch label={`${name} 限定检测区间`} checked={!!range} disabled={disabled}
        onCheckedChange={on => onPatch({ inspectRange: on ? { aMm: 0, bMm: segment.lengthMm ?? 100 } : null })} />
      {range && <>
        <NumberField label="检测起点 a" unit="mm" aria-label={`${name} 检测起点`} step={1} min={0} disabled={disabled}
          value={Number.isFinite(range.aMm) ? range.aMm : ""} onValueChange={value => onPatch({ inspectRange: { ...range, aMm: value === "" ? NaN : value } })} />
        <NumberField label="检测终点 b" unit="mm" aria-label={`${name} 检测终点`} step={1} min={0} disabled={disabled}
          value={Number.isFinite(range.bMm) ? range.bMm : ""} onValueChange={value => onPatch({ inspectRange: { ...range, bMm: value === "" ? NaN : value } })} />
      </>}
    </div>
    {straight && segment.checks.offset?.enabled && <div className="seg-offset" role="group" aria-label={`${name} 胶偏参考`}>
      <Select label="胶偏测量量" aria-label={`${name} 胶偏测量量`} value={measure} options={measureOptions} disabled={disabled} onValueChange={value => setReference({ measure: value as OffsetMeasure })} />
      <Select label="参考相机" aria-label={`${name} 参考相机`} value={camera} options={inspecting.map(id => ({ value: id, label: id }))} disabled={disabled} onValueChange={value => setReference({ camera: value })} />
      <p className={offsetRef?.xRef == null ? "muted" : "plan-mono"}>{offsetRef?.xRef != null
        ? `x_ref ${offsetRef.xRef} ± ${offsetRef.spread ?? "—"} mm（标准件多轮测量均值）`
        : measure === "toEdge" ? "到边缘距离的测量量本轮未实现：该段胶偏不判" : "x_ref 待标定：由标准件多轮采集得出，不能手填"}</p>
    </div>}
    <details className="seg-checks">
      <summary>{name}检测项（{segmentKindLabel[segment.kind]}）</summary>
      <KindChecksEditor value={segment.checks} kind={segment.kind} disabled={disabled} onChange={setChecks} />
    </details>
  </section></li>;
}
