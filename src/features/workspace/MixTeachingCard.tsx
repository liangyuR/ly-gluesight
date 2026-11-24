import { useEffect, useRef, useState } from "react";
import { Button, Dialog, NumberField, TextField } from "qing-steel";
import { desktopCall } from "../../lib/desktop";
import { errorText } from "../../lib/errors";
import type { MixCheck } from "../plan/types";
import { decodeFrameImage, workspaceApi } from "./api";
import { FrameViewer, Notice } from "./components";
import type { FrameImage, HistoryCandidate, WorkspaceView } from "./types";

type Rect = [number, number, number, number];
type Endpoints = { yellow: string | null; blue: string | null };
type Sample = { token: string; greenLines: [[number, number], [number, number]][] };
type Example = { historyId: number; seq: number; greenRects: Rect[] };
type Source = { kind: "software" } | { kind: "history"; historyId: number; seq: number; segment: number };

interface Props {
  id: string;
  revision: number;
  camera: string;
  segment: number;
  check?: MixCheck | null;
  disabled: boolean;
  pending: boolean;
  change: (check?: MixCheck) => void;
  persist: (request: () => Promise<WorkspaceView>, message: string) => Promise<WorkspaceView | null>;
}

const initial: MixCheck = { backgroundRects: [], maxLengthMm: 3, maxAreaMm2: 3, driftLengthMm: 5 };

export default function MixTeachingCard(props: Props) {
  const { check, change, disabled, pending } = props;
  const [open, setOpen] = useState(false);
  const [confirmOff, setConfirmOff] = useState(false);
  return <section className="wp-settings-group" aria-label="混胶色差">
    <h3>混胶色差</h3>
    {check && <p>{check.model ? (check.model.datasetKind === "synthetic" ? "合成联调模型" : "已示教") : "已启用，待示教"}</p>}
    <div className="wp-actions">
      <Button size="sm" disabled={disabled} onClick={() => check?.model ? setConfirmOff(true) : change(check ? undefined : { ...initial })}>
        {check ? "关闭混胶色差" : "启用混胶色差"}
      </Button>
      {check && <Button size="sm" disabled={disabled || pending} onClick={() => setOpen(true)}>示教混胶色差</Button>}
    </div>
    {check && <>
      {(["maxLengthMm", "maxAreaMm2", "driftLengthMm"] as const).map((key, i) =>
        <NumberField key={key} label={["异色条纹长度门限", "异色斑面积门限", "持续偏色长度门限"][i]}
          unit={key === "maxAreaMm2" ? "mm²" : "mm"} value={check[key]} min={0.001} step={0.1} disabled={disabled}
          onValueChange={v => { if (typeof v === "number" && v > 0) change({ ...check, [key]: v }); }} />)}
    </>}
    {open && check && <TeachingDialog {...props} check={check} close={() => setOpen(false)} />}
    {confirmOff && <Dialog open title="关闭本段混胶色差？" onOpenChange={o => { if (!o) setConfirmOff(false); }}
      footer={<><Button onClick={() => setConfirmOff(false)}>取消</Button><Button onClick={() => { change(undefined); setConfirmOff(false); }}>关闭并清除本段模型</Button></>}>
      再次启用时需要重新拟合本段模型。相机的纯黄、纯蓝取样可继续使用。
    </Dialog>}
  </section>;
}

function TeachingDialog({ id, revision, camera, segment, check, close, persist, disabled, pending }: Props & { check: MixCheck; close: () => void }) {
  const [ends, setEnds] = useState<Endpoints>({ yellow: null, blue: null });
  const [rows, setRows] = useState<HistoryCandidate[]>([]);
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState(0);
  const [seq, setSeq] = useState(1);
  const [image, setImage] = useState<FrameImage | null>(null);
  const [sample, setSample] = useState<(Sample & { source: Source }) | null>(null);
  const [rect, setRect] = useState<Rect | null>(null);
  const [background, setBackground] = useState<Rect[]>(check.backgroundRects);
  const [examples, setExamples] = useState<Example[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const fileInput = useRef<HTMLInputElement>(null);
  const current = useRef(true);
  const openedRevision = useRef(revision).current;
  const locked = disabled || pending || busy || openedRevision !== revision;
  useEffect(() => {
    current.current = true;
    desktopCall<Endpoints>("workspace_mix_endpoints", { camera }).then(e => { if (current.current) setEnds(e); })
      .catch(e => { if (current.current) setError(errorText(e)); });
    return () => { current.current = false; };
  }, [camera]);
  useEffect(() => {
    let alive = true;
    workspaceApi.historyCandidates(id, { sn: search, limit: 50 }).then(page => {
      if (alive) setRows(page.items.filter(row => row.cameras.some(c => c.cameraId === camera)));
    }).catch(e => { if (alive) setError(errorText(e)); });
    return () => { alive = false; };
  }, [id, camera, search]);
  async function run(action: () => Promise<void>) {
    if (locked) return;
    setBusy(true); setError(""); setNotice("");
    try { await action(); } catch (e) { if (current.current) setError(errorText(e)); }
    finally { if (current.current) setBusy(false); }
  }
  async function freeze(source: Source) {
    setSample(null); setImage(null); setRect(null);
    const s = await desktopCall<Sample>("workspace_mix_sample", { camera, source });
    const frame = await desktopCall<ArrayBuffer>("workspace_mix_sample_image", { camera, token: s.token }).then(decodeFrameImage);
    if (current.current) { setSample({ ...s, source }); setImage(frame); }
  }
  const usableRect = rect && rect[2] > 0 && rect[3] > 0 ? rect : null;
  const saveEndpoint = (role: keyof Endpoints) => run(async () => {
    if (!sample || !usableRect) return;
    const e = await desktopCall<Endpoints>("workspace_mix_save_endpoint", { camera, token: sample.token, role, rect: usableRect });
    setEnds(e); setNotice(role === "yellow" ? "已保存纯黄端点" : "已保存纯蓝端点");
  });
  const addFrame = () => {
    if (sample?.source.kind !== "history") return;
    const { historyId, seq } = sample.source;
    const greenRects = usableRect ? [usableRect] : [];
    setExamples(prev => [...prev.filter(e => e.historyId !== historyId || e.seq !== seq), { historyId, seq, greenRects }]);
    setRect(null); setNotice("已加入合格样本");
  };
  const importModel = (file: File) => run(async () => {
    const reference = JSON.parse(await file.text());
    const backgroundRects = background.length ? background : reference.backgroundRects;
    if (!Array.isArray(backgroundRects) || !backgroundRects.length) throw Error("请先选择背景取样区，或从离线评估导入带背景框的模型");
    if (await persist(() => desktopCall("workspace_mix_import", { id, revision, camera, segment, reference, backgroundRects }), "已导入合成联调模型")) close();
  });
  return <Dialog open size="lg" title={`${camera} · 第 ${segment} 段混胶色差示教`} closeDisabled={busy}
    onOpenChange={o => { if (!o && !busy) close(); }} footer={<>
      <Button disabled={busy} onClick={close}>关闭</Button>
      <Button variant="filled" disabled={locked || !examples.length || !background.length || !ends.yellow || !ends.blue}
        onClick={() => void run(async () => {
          if (await persist(() => desktopCall("workspace_mix_fit", { id, revision, camera, segment, examples, backgroundRects: background, endpoints: ends }), "混胶模型已拟合并保存")) close();
        })}>拟合并保存模型</Button></>}>
    <div className="wp-stack">
      <div className="wp-actions"><Button disabled={locked} onClick={() => void run(() => freeze({ kind: "software" }))}>Software 拍摄排胶样</Button>
        <Button disabled={locked} onClick={() => fileInput.current?.click()}>导入合成联调模型</Button>
        <input type="file" ref={fileInput} hidden accept=".json" onChange={e => { const f = e.target.files?.[0]; e.target.value = ""; if (f) void importModel(f); }} /></div>
      <TextField label="按 SN 查找合格历史件" value={search} onChange={e => setSearch(e.target.value)} />
      <div className="wp-actions"><label>历史件 <select aria-label="合格历史件" value={selected} onChange={e => { setSelected(Number(e.target.value)); setSeq(1); }}>
        <option value={0}>选择工件</option>{rows.map(r => <option key={r.recordId} value={r.recordId}>{r.sn} · {r.projectId} / {r.recipeId ?? "无配方"} · #{r.recordId}</option>)}</select></label>
        <NumberField label="帧号" min={1} step={1} value={seq} onValueChange={v => setSeq(Math.max(1, Math.trunc(Number(v))))} />
        <Button disabled={locked || !rows.some(r => r.recordId === selected)} onClick={() => void run(() => freeze({ kind: "history", historyId: selected, seq, segment }))}>读取合格帧</Button></div>
      {image && <>
        <FrameViewer image={image} label="混胶取样原图" region={rect} onRegion={r => setRect(r.map(Math.round) as Rect)}
          overlay={sample?.greenLines.length ? { path: [], partEdges: sample.greenLines } : undefined} />
        <div className="wp-actions">
          {sample?.source.kind === "software" && <>{(["yellow", "blue"] as const).map(role => <Button key={role} disabled={locked || !usableRect}
            onClick={() => void saveEndpoint(role)}>{role === "yellow" ? "框选为纯黄" : "框选为纯蓝"}</Button>)}</>}
          {sample?.source.kind === "history" && <><Button disabled={locked || (!usableRect && !sample.greenLines.length)} onClick={addFrame}>
            {usableRect ? "加入框选绿胶" : "加入完整胶边内的绿胶"}</Button>
            <Button disabled={locked || !usableRect} onClick={() => { if (usableRect) setBackground([...background, usableRect]); setRect(null); }}>加入背景框</Button>
            {rect && <Button onClick={() => setRect(null)}>清除框选</Button>}</>}
        </div>
      </>}
      <p>纯黄端点{ends.yellow ? "已保存" : "待取样"} · 纯蓝端点{ends.blue ? "已保存" : "待取样"} · 已选 {examples.length} 帧合格样本</p>
      {examples.map(e => <div key={`${e.historyId}-${e.seq}`}>工件 #{e.historyId} · 帧 {e.seq} · {e.greenRects.length ? "手动胶面" : "完整胶边"}
        <Button size="sm" onClick={() => setExamples(examples.filter(item => e !== item))}>移除</Button></div>)}
      {background.map((r, i) => <div key={i}>背景框 {i + 1} · {r.join(", ")} px <Button size="sm" onClick={() => setBackground(background.filter((_, n) => n !== i))}>移除</Button></div>)}
      {examples.length > 0 && new Set(examples.map(e => e.historyId)).size === 1 && <p>目前仅一个合格工件，批次覆盖不足。</p>}
      {openedRevision !== revision && <Notice tone="warn" title="候选修订已改变">请关闭窗口，在当前修订重新确认取样。</Notice>}
      {notice && <p role="status">{notice}</p>}{error && <Notice tone="warn" title="混胶示教未完成">{error}</Notice>}
    </div>
  </Dialog>;
}
