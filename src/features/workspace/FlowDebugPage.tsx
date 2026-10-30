import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { ArrowRight, RotateCcw, Save, Workflow } from "lucide-react";
import { Button, EmptyState, Select, Surface, useSnackbar } from "qing-steel";
import type { GraphDoc } from "@lyflow/editor";
import { errorText } from "../../lib/errors";
import type { RecipeDoc } from "../cycle/types";
import { segmentKindLabel } from "../plan/vis";
import { useWorkspace } from "./context";
import { Badge, Notice, NumberField, WorkspaceBar, WorkspaceEmpty } from "./components";
import { flowChanges, type FlowChanges } from "./flowDebug";
import { flowGraphPath, FlowTransport, openFlow, type FlowVariant } from "./flowTransport";
import { confirmedSegmentRange } from "./teach";
import type { WorkspaceView } from "./types";

const FlowEditor = lazy(() => import("./FlowEditor"));

const variantLabel: Record<FlowVariant, string> = { corridor: "示教走廊", heading: "逐帧定向" };
const noChanges: FlowChanges = { expert: {}, ignored: [] };

export default function FlowDebugPage() {
  const { data, doc } = useWorkspace();
  if (!data || !doc) return <WorkspaceEmpty />;
  return <FlowDebug data={data} doc={doc} />;
}

/**
 * 流程调试（LF-6）：在示教件的一帧上运行本段、本相机的测量图，逐节点看输出。拓扑锁定，只调参数；
 * 改过的专家参数可存为本相机覆盖，其余改动只在本页试跑。切换相机、段、测量图或帧组时编辑器卸载重挂。
 */
function FlowDebug({ data, doc }: { data: WorkspaceView; doc: RecipeDoc }) {
  const { setDoc } = useWorkspace();
  const { show: snack } = useSnackbar();
  const [search, setSearch] = useSearchParams();
  const plan = data.plan;
  const saved = data.workspace.doc;
  const entries = plan.cameraTeaching.flatMap(item => item.segments.filter(s => s.saved).map(s => ({ camera: item.camera, segment: s.segment, round: s.round, seq: s.seq })));
  const cameras = [...new Set(entries.map(item => item.camera))];
  const camera = cameras.find(item => item === search.get("camera")) ?? cameras[0] ?? "";
  const own = entries.filter(item => item.camera === camera);
  const entry = own.find(item => item.segment === Number(search.get("segment"))) ?? own[0];
  const segment = entry?.segment ?? 0;
  const headingOn = !!saved.segments?.find(item => item.index === segment)?.heading;
  const variants: FlowVariant[] = headingOn ? ["corridor", "heading"] : ["corridor"];
  const variant = variants.find(item => item === search.get("variant")) ?? "corridor";
  const review = entry ? data.workspace.captureReview?.rounds[entry.round] : undefined;
  const range = !entry ? null : plan.segments.length === 1 ? (review ? { start: 1, end: review.groupCount } : null) : confirmedSegmentRange(review, plan.segments, segment);
  const asked = Number(search.get("seq"));
  const seq = Number.isInteger(asked) && range && asked >= range.start && asked <= range.end ? asked : entry?.seq ?? 1;
  const target = entry ? { id: doc.id, camera, segment, round: entry.round, seq, variant } : null;
  const path = target ? flowGraphPath(target) : "";

  // 选择记在地址里：换相机回到它的第一段，换段回到示教走廊与示教帧
  const choose = (next: { camera?: string; segment?: number; variant?: string; seq?: number }) => {
    const params: Record<string, string> = { camera: next.camera ?? camera };
    if (next.camera == null) params.segment = String(next.segment ?? segment);
    if (next.camera == null && next.segment == null) Object.assign(params, { variant: next.variant ?? variant, seq: String(next.seq ?? seq) });
    setSearch(params, { replace: true });
  };

  // 帧组输入停顿后才重开，输 12 不会先开 1
  const [draftSeq, setDraftSeq] = useState<number | null>(null);
  useEffect(() => {
    if (draftSeq == null || draftSeq === seq || !range || !Number.isInteger(draftSeq) || draftSeq < range.start || draftSeq > range.end) return;
    const timer = setTimeout(() => choose({ seq: draftSeq }), 500);
    return () => clearTimeout(timer);
  }, [draftSeq]);
  useEffect(() => setDraftSeq(null), [seq]);

  const [reload, setReload] = useState(0);
  const [opened, setOpened] = useState<{ key: string; transport: FlowTransport | null; name: string; error: string }>({ key: "", transport: null, name: "", error: "" });
  const key = `${path}#${reload}`;
  useEffect(() => {
    if (!target) return;
    let alive = true;
    setOpened({ key, transport: null, name: "", error: "" });
    openFlow(target).then(loaded => alive && setOpened({ key, transport: new FlowTransport(path, loaded), name: loaded.doc.name ?? "", error: "" }))
      .catch(e => alive && setOpened({ key, transport: null, name: "", error: errorText(e) }));
    return () => { alive = false; };
  }, [key]);
  const ready = opened.key === key ? opened : { transport: null, name: "", error: "" };

  // 基线取编辑器打开后的第一份文档（它按算子默认值稀疏化过节点参数），之后的每一份与它比
  const baseline = useRef<GraphDoc | null>(null);
  const latest = useRef<GraphDoc | null>(null);
  const [changes, setChanges] = useState<FlowChanges>(noChanges);
  useEffect(() => { baseline.current = null; latest.current = null; setChanges(noChanges); }, [key]);
  const docChanged = useCallback((next: GraphDoc, dirty: boolean) => {
    if (!baseline.current) {
      if (dirty || next.name !== ready.name) return;
      baseline.current = next;
    }
    latest.current = next;
    setChanges(flowChanges(baseline.current, next));
  }, [ready.name]);

  const expertCount = Object.keys(changes.expert).length;
  const saveReason = data.workspace.pending ? "候选版本正在等待生效" : !expertCount ? "没有改动的专家参数" : "";
  const saveExpert = () => {
    const base = baseline.current, now = latest.current;
    if (saveReason || !base || !now) return;
    const { expert, ignored } = flowChanges(base, now);
    setDoc({ ...doc, inspect: (doc.inspect ?? []).map(item => item.camera === camera ? { ...item, expert: { ...item.expert, ...expert } } : item) });
    baseline.current = { ...base, params: Object.fromEntries(Object.entries(base.params ?? {}).map(([name, spec]) => [name, name in expert ? { ...spec, default: expert[name] } : spec])) };
    setChanges(flowChanges(baseline.current, now));
    snack(`已把 ${Object.keys(expert).length} 项专家参数存为 ${camera} 的本相机覆盖；发布前请重新试测或跑样本验证` + (ignored.length ? `。${ignored.join("、")} 不是专家参数，未保存` : ""));
  };

  if (!entries.length) return <div className="wp-page"><WorkspaceBar /><Surface><EmptyState icon={<Workflow size={32} aria-hidden="true" />} title="还没有可调试的测量图" description="先在走廊示教里为至少一段保存走廊，再回来逐节点调试。"
    action={<Button asChild><Link to="/recipe/teach">前往走廊示教<ArrowRight size={15} aria-hidden="true" /></Link></Button>} /></Surface></div>;

  return <div className="wp-page wp-flow-page"><WorkspaceBar />
    <Surface role="region" className="wp-flow-bar" aria-label="调试对象">
      <Select label="相机" value={camera} options={cameras.map(id => ({ value: id, label: id }))} onValueChange={value => choose({ camera: value })} />
      <Select label="段" value={String(segment)} options={own.map(item => {
        const kind = plan.segments.find(s => s.index === item.segment)?.kind;
        return { value: String(item.segment), label: `段 ${item.segment}${kind ? " · " + segmentKindLabel[kind] : ""}` };
      })} onValueChange={value => choose({ segment: Number(value) })} />
      <Select label="测量图" value={variant} options={variants.map(item => ({ value: item, label: variantLabel[item] }))} onValueChange={value => choose({ variant: value })} />
      <NumberField label={range ? `帧组（${range.start}–${range.end}）` : "帧组"} value={draftSeq ?? seq} min={range?.start} max={range?.end} disabled={!range} onChange={setDraftSeq} />
      <Button variant="text" size="sm" disabled={!entry || seq === entry.seq} onClick={() => entry && choose({ seq: entry.seq })}>回到示教帧</Button>
      <span className="wp-flow-actions">
        {seq === entry?.seq && <Badge tone="info">示教帧</Badge>}
        {expertCount > 0 && <Badge tone="warn">{expertCount} 项专家参数已改</Badge>}
        <Button variant="text" size="sm" leadingIcon={<RotateCcw size={14} />} onClick={() => setReload(value => value + 1)}>按候选重新打开</Button>
        <Button variant={expertCount ? "filled" : "tonal"} size="sm" leadingIcon={<Save size={14} />} disabled={!!saveReason} title={saveReason || undefined} onClick={saveExpert}>保存为本相机专家参数</Button>
      </span>
    </Surface>
    <p className="muted wp-flow-hint">拓扑锁定，只能调参数。参数按候选已保存的配方合成；只有标为专家参数的顶层图参数能存为 {camera} 的本相机覆盖，节点参数与其余图参数的改动只在本页试跑。切换相机、段、测量图或帧组会重新打开，未保存的改动丢弃。
      {changes.ignored.length > 0 && <span className="c-warn"> 不会保存：{changes.ignored.join("、")}。</span>}</p>
    {ready.error && <Notice title="打开测量图失败" tone="warn">{ready.error}</Notice>}
    <div className="wp-flow-host">
      {ready.transport
        ? <Suspense fallback={<p className="muted center">正在加载流程编辑器…</p>}><FlowEditor key={key} transport={ready.transport} path={path} onDocChange={docChanged} /></Suspense>
        : !ready.error && <p className="muted center">正在打开测量图…</p>}
    </div>
  </div>;
}
