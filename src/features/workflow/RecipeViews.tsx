import { useEffect, useRef, useState } from "react";
import { ArrowRight, Camera, Check, ChevronRight, Copy, FileImage, Plus, RotateCcw, Save, ShieldCheck, Trash2, Upload, WandSparkles } from "lucide-react";
import { useWorkflow } from "./context";
import { canPublish, canSaveFrame, coverage, initialPositions, sampleVerdict, validationChecks, validationSamples } from "./model";
import { Badge, BusyLabel, Dialog, FrameCanvas, FrameList, KV, Notice, NumberField, OverviewMap, Panel, SelectField, Steps, VerdictBadge, useTask } from "./components";

export function RecipesView() {
  const { state: s, dispatch, go, notify } = useWorkflow();
  const [query, setQuery] = useState("");
  const [dialog, setDialog] = useState<"new" | "copy" | null>(null);
  const [name, setName] = useState("");
  const [deleting, setDeleting] = useState<string | null>(null);
  const locked = s.live.phase > 0 && s.live.phase < 4 || s.live.queued !== null;
  const entries = s.recipeLibrary.map(entry => ({ ...entry.recipe, active: entry.recipe.name === s.recipe.name })).filter(e => e.name.toLowerCase().includes(query.toLowerCase()));
  const duplicate = s.recipeLibrary.some(entry => entry.recipe.name === name.trim());
  const edit = () => {
    if (s.recipe.candidate === s.recipe.production) dispatch({ type: "recipe", patch: { candidate: s.recipe.production + 1 } });
    go("geometry");
  };
  const create = () => {
    if (!name.trim() || duplicate || locked) return;
    dispatch({ type: "recipe-create", name, copy: dialog === "copy" });
    setDialog(null); notify(dialog === "copy" ? "配方参数已复制，图像示教需要重新确认。" : "新配方已建立，请规划胶路与视野。"); go("geometry");
  };
  return <>
    <div className="wf-page-actions"><input className="input wf-search" aria-label="搜索配方" placeholder="搜索产品 / 配方名称" value={query} onChange={e => setQuery(e.target.value)} /><div className="wf-row"><button className="btn" disabled={locked} onClick={() => { setDialog("copy"); setName(s.recipe.name + " 副本"); }}><Copy size={15} />复制当前配方</button><button className="btn primary" disabled={locked} onClick={() => { setDialog("new"); setName(""); }}><Plus size={15} />新建配方</button></div></div>
    {locked && <Notice title="本件完成后再切换或删除配方" tone="warn">当前工件与已排队版本分别保留完整配置，仍可查看当前候选。</Notice>}
    <Panel title="产品配方" detail="进入候选版本修改，验证后再更新生产版本"><div className="wf-table-wrap"><table className="table wf-table"><thead><tr><th>配方名称</th><th>生产版本</th><th>候选版本</th><th>状态</th><th>操作</th></tr></thead><tbody>{entries.map(e => <tr key={e.name}><td><strong>{e.name}</strong>{e.active && <span className="wf-table-sub">当前工作台</span>}</td><td>v{e.production}</td><td>v{e.candidate}</td><td><Badge tone={e.active ? "info" : "neutral"}>{e.active ? s.validation.status === "passed" ? "候选已验证" : "候选待验证" : "已停用"}</Badge></td><td><button className="btn" onClick={() => {
      if (!e.active && locked) return;
      dispatch({ type: "recipe-open", name: e.name }); go("geometry");
    }} disabled={locked && !e.active}>编辑候选<ChevronRight size={14} /></button><button className="btn" aria-label={"删除配方 " + e.name} disabled={locked || s.recipeLibrary.length <= 1} onClick={() => setDeleting(e.name)}><Trash2 size={14} />删除</button></td></tr>)}</tbody></table>{entries.length === 0 && <div className="empty small">没有匹配的配方</div>}</div></Panel>
    <div className="wf-two-col"><Panel title="当前生产配置" detail="在线工件使用这个版本"><div className="wf-version"><span>生产版本</span><strong>{s.recipe.production > 0 ? "v" + s.productionConfig.version : "未发布"}</strong><Badge tone={s.recipe.production > 0 ? "ok" : "warn"}>{s.recipe.production > 0 ? "已生效" : "待发布"}</Badge></div><KV label="产品">{s.productionConfig.recipe.name}</KV><KV label="标定版本">{s.productionConfig.calibrationVersion}</KV><p className="wf-caption">编辑候选配方不会立即改变生产配置。</p></Panel><Panel title="候选配置" detail="修改、示教、验证后发布"><div className="wf-version"><span>候选版本</span><strong>v{s.recipe.candidate}</strong><Badge tone={s.validation.status === "passed" ? "ok" : "warn"}>{s.validation.status === "passed" ? "验证通过" : "待验证"}</Badge></div><KV label="已保存示教">{s.frames.filter(f => f.saved && canSaveFrame(f)).length} / 6 帧</KV><KV label="物理覆盖">{coverage(s)}%</KV><div className="wf-actions"><button className="btn primary" onClick={edit}>继续配置<ArrowRight size={15} /></button><button className="btn" onClick={() => go("validation")}>查看发布条件</button></div></Panel></div>
    {dialog && <Dialog title={dialog === "new" ? "新建配方" : "复制配方"} onClose={() => setDialog(null)}><div className="stack"><label className="wf-field"><span>配方名称</span><input className="input" autoFocus value={name} onChange={e => setName(e.target.value)} placeholder="输入产品 / 配方名称" /></label><Notice title={dialog === "copy" ? "复制参数，重新确认样本" : "从候选版本开始"}>{dialog === "copy" ? "图像、标定与工件位置需要在新配方下重新验证。" : "完成配置与验证后，配方才能发布为生产版本。"}</Notice>{duplicate && <Notice title="配方名称已存在" tone="warn">请输入不同的名称。</Notice>}</div><div className="wf-dialog-actions"><button className="btn" onClick={() => setDialog(null)}>取消</button><button className="btn primary" disabled={!name.trim() || duplicate || locked} onClick={create}>建立配方</button></div></Dialog>}
    {deleting && <Dialog title="删除配方" onClose={() => setDeleting(null)}><Notice title={"删除「" + deleting + "」？"} tone="warn">此预览配方的候选参数和示教将移除。删除当前配方后会打开剩余配方。</Notice><div className="wf-dialog-actions"><button className="btn" onClick={() => setDeleting(null)}>取消</button><button className="btn primary" disabled={locked || s.recipeLibrary.length <= 1} onClick={() => { dispatch({ type: "recipe-delete", name: deleting }); setDeleting(null); notify("预览配方已删除。"); }}>确认删除</button></div></Dialog>}
  </>;
}

export function GeometryView() {
  const { state: s, dispatch, go } = useWorkflow();
  const g = s.recipe;
  const locked = s.live.phase > 0 && s.live.phase < 4 && g.candidate === g.production;
  const cov = coverage(s);
  const blur = g.speed * s.device.exposure / 1e6 / 0.04;
  const perimeter = 2 * (g.width + g.height - 4 * g.radius) + 2 * Math.PI * g.radius;
  const interval = g.speed > 0 ? perimeter / 6 / g.speed * 1000 : 0;
  const edit = (key: "width" | "height" | "radius" | "fovWidth" | "fovHeight", value: number) => dispatch({ type: "recipe", patch: { [key]: value }, geometry: true });
  return <>
    {locked && <Notice title="当前生产参数暂时锁定" tone="warn">完成本件或创建候选版本后再修改。</Notice>}<Steps active="geometry" /><div className="wf-page-actions"><div className="wf-row"><Badge>候选 v{g.candidate}</Badge><Badge tone={cov !== 100 ? "warn" : "ok"}>{"覆盖 " + cov + "%"}</Badge></div><button className="btn primary" onClick={() => go("teach")}>下一步 · 单帧示教<ChevronRight size={15} /></button></div>
    <div className="wf-two-col wf-wide-left"><div className="stack"><Panel title="胶路与六帧规划" detail="示例几何检查：胶路和搜索余量共同进入视野"><OverviewMap physical /><div className="wf-stat-grid"><div><span>胶路周长</span><strong>{perimeter.toFixed(1)} <small>mm</small></strong></div><div><span>拍照帧数</span><strong>6 帧</strong></div><div><span>示例比例</span><strong>0.040 <small>mm/px</small></strong></div></div></Panel>
    <Panel title="采集可行性" detail="按当前曝光、速度和示例比例估算"><div className="wf-stat-grid"><div><span>曝光运动模糊</span><strong className={blur > 1 ? "wf-warn-text" : "ok"}>{blur.toFixed(2)} <small>px</small></strong></div><div><span>平均触发间隔</span><strong>{interval.toFixed(0)} <small>ms</small></strong></div><div><span>示例单帧耗时</span><strong>179 <small>ms</small></strong></div></div>{cov < 100 ? <Notice title={"胶路搜索窗口覆盖不足 · " + cov + "%"} tone="warn">扩大视野或调整物理拍照位置，覆盖完整后才能通过验证。总览显示框的拖动不改变此结果。</Notice> : blur > 1 || interval < 179 ? <Notice title="当前采集条件需要调整" tone="warn">降低运动速度或曝光时间，并检查真实设备采集节拍。</Notice> : <Notice title="当前示例规划可行" tone="ok">胶路及搜索余量全部被覆盖，采集时间留有余量。</Notice>}</Panel></div>
    <div className="stack"><Panel title="工件与视野"><div className="wf-form-grid"><NumberField disabled={locked} label="工件宽度" value={g.width} min={100} max={2000} unit="mm" onChange={v => edit("width", v)} /><NumberField disabled={locked} label="工件高度" value={g.height} min={100} max={1200} unit="mm" onChange={v => edit("height", v)} /><NumberField disabled={locked} label="圆角半径" value={g.radius} min={0} max={Math.min(g.width, g.height) / 2} unit="mm" onChange={v => edit("radius", v)} /><NumberField disabled={locked} label="物理视野宽" value={g.fovWidth} min={50} max={1500} unit="mm" onChange={v => edit("fovWidth", v)} /><NumberField disabled={locked} label="物理视野高" value={g.fovHeight} min={50} max={1500} unit="mm" onChange={v => edit("fovHeight", v)} /><NumberField disabled={locked} label="运动速度" value={g.speed} min={10} max={2000} unit="mm/s" onChange={v => dispatch({ type: "recipe", patch: { speed: v } })} /></div><p className="wf-caption wf-gap-top">工件、视野或标定修改后，相关帧需要重新取样。</p></Panel><Panel title="判定标准" detail="距离 d 与胶宽 w 分别设置"><div className="wf-form-grid"><NumberField disabled={locked} label="距内边基准 d" value={g.target} min={0} max={30} unit="mm" step={0.1} onChange={v => dispatch({ type: "recipe", patch: { target: v } })} /><NumberField disabled={locked} label="距离容差" value={g.tolerance} min={0.1} max={10} unit="± mm" step={0.1} onChange={v => dispatch({ type: "recipe", patch: { tolerance: v } })} /><NumberField disabled={locked} label="允许断胶长度" value={g.maxGap} min={0} max={30} unit="mm" step={0.1} onChange={v => dispatch({ type: "recipe", patch: { maxGap: v } })} /></div><KV label="本帧胶宽范围">{s.frames[s.selectedFrame - 1].params.minWidth}–{s.frames[s.selectedFrame - 1].params.maxWidth} mm</KV><p className="wf-caption">胶宽与搜索参数在单帧示教中确认。</p></Panel></div></div>
  </>;
}

export function TeachView() {
  const { state: s, dispatch, go, notify } = useWorkflow();
  const [quality, setQuality] = useState("完整胶路");
  const task = useTask();
  const f = s.frames[s.selectedFrame - 1];
  const saveable = canSaveFrame(f);
  const status = f.imageId === null ? "待重新取样" : f.saved ? "本帧已保存" : f.trial ? f.trial.pass ? "试匹配通过" : "试匹配失败" : "待试匹配";
  const save = (next: boolean) => {
    if (!saveable) return;
    dispatch({ type: "save-frame" });
    notify("k" + f.id + " 示教已保存，整套配方仍需批量验证。");
    if (next) { if (f.id < 6) dispatch({ type: "select-frame", id: f.id + 1 }); else go("overview"); }
  };
  return <>
    {task.error && <Notice title="预览操作失败" tone="warn">{task.error}</Notice>}
    <Steps active="teach" /><div className="wf-page-actions"><div className="wf-row"><Badge>候选 v{s.recipe.candidate}</Badge><Badge tone={f.saved ? "ok" : f.trial && !f.trial.pass ? "warn" : "neutral"}>{status}</Badge><span className="wf-caption">已保存 {s.frames.filter(fr => fr.saved && canSaveFrame(fr)).length} / 6 帧</span></div><button className="btn" onClick={() => go("overview")}>布置总览<ChevronRight size={15} /></button></div>
    <div className="wf-teach-grid"><Panel title={s.recipe.name + " / 相机 1"} detail="每帧独立取样与示教" className="wf-frame-panel"><FrameList frames={s.frames} selected={f.id} onSelect={id => dispatch({ type: "select-frame", id })} disabled={task.busy} /><button className="btn wf-full" onClick={() => go("geometry")}>查看拍照规划</button></Panel>
      <div className="stack"><Panel title={"k" + f.id + " · 单帧图像"} detail={f.source === "history" ? "历史 " + f.sourceRecord + " · 原始记录保留" : "冻结图像与本帧参数绑定"} actions={<button className="btn" disabled={!s.device.connected || !s.device.applied || task.busy} onClick={() => { dispatch({ type: "capture", quality: quality === "完整胶路" ? "normal" : "low" }); notify("已冻结 k" + f.id + " 新样本，等待试匹配。"); }}><Camera size={15} />取新样本</button>}><FrameCanvas id={f.id} imageId={f.imageId} overlay={f.trial !== null || f.saved} failed={f.trial?.pass === false} label={f.source === "history" ? "历史样本" : "冻结样本"} /><div className="wf-sample-line"><SelectField label="取样情景" value={quality} options={["完整胶路", "低对比样本"]} onChange={setQuality} disabled={task.busy} /><div className="wf-caption"><div>相机 SN-0001 · 曝光 {f.captureSettings.exposure} μs</div><div>标定 {f.captureSettings.calibrationVersion} · 参数修订 {f.revision}</div></div></div></Panel>
      <Panel title="本帧试匹配结果" className="wf-teach-result" actions={f.trial && <Badge tone={f.trial.pass ? "ok" : "warn"}>{f.trial.pass ? "通过" : "定位失败"}</Badge>}>
        <div className="wf-actions"><button className="btn" disabled={f.imageId === null || task.busy} onClick={() => task.run(() => dispatch({ type: "trial" }))}><BusyLabel busy={task.busy}><WandSparkles size={15} />试匹配当前帧</BusyLabel></button><button className="btn primary" disabled={!saveable || f.saved || task.busy} onClick={() => save(false)}><Save size={15} />保存本帧示教</button><button className="btn" disabled={!saveable || task.busy} onClick={() => save(true)}>{f.id < 6 ? "保存并示教下一帧" : "保存并进入总览"}<ChevronRight size={15} /></button></div>
        {f.imageId === null ? <Notice title={s.scene === "teach-stale" ? "采集参数已变更，请重新取样" : "先冻结本帧图像"} tone="warn">没有有效冻结图像，试匹配和保存暂不可用。</Notice> : f.trial ? f.trial.pass ? <><div className="wf-stat-grid"><div><span>匹配质量</span><strong>{f.trial.score.toFixed(2)}</strong></div><div><span>有效覆盖</span><strong>100 <small>%</small></strong></div><div><span>示例胶宽 w</span><strong>3.94 <small>mm</small></strong></div></div><p className="wf-caption">{f.saved ? "本帧示教已保存。保存本帧后仍需验证整套配方。" : "当前图像与参数已通过试匹配，可以保存本帧示教。"}</p></> : <Notice title="定位失败，当前样本不能保存" tone="warn">示例质量评分 0.31。检查胶路对比度、搜索窗口和图像条件，重新取样或调整参数后重试。</Notice> : <Notice title={f.revision > 1 ? "本帧参数已修改，请重新试匹配" : "冻结样本已就绪"}>试匹配只检查当前图像与当前参数，结果通过后才能保存。</Notice>}
      </Panel></div>
      <div className="stack"><Panel title="本帧参数" detail="修改后必须重新试匹配"><div className="wf-form-grid"><NumberField label="搜索窗口余量" value={f.params.search} min={0.5} max={20} unit="mm" step={0.5} disabled={task.busy} onChange={v => dispatch({ type: "frame-param", key: "search", value: v })} /><NumberField label="最低灰度对比" value={f.params.contrast} min={1} max={255} unit="级" disabled={task.busy} onChange={v => dispatch({ type: "frame-param", key: "contrast", value: v })} /><NumberField label="胶宽下限 w" value={f.params.minWidth} min={0.1} max={20} unit="mm" step={0.1} disabled={task.busy} onChange={v => dispatch({ type: "frame-param", key: "minWidth", value: v })} /><NumberField label="胶宽上限 w" value={f.params.maxWidth} min={0.1} max={20} unit="mm" step={0.1} disabled={task.busy} onChange={v => dispatch({ type: "frame-param", key: "maxWidth", value: v })} /></div>{f.params.minWidth >= f.params.maxWidth && <Notice title="胶宽上限必须大于下限" tone="warn" />}</Panel><Panel title="样本绑定"><KV label="工件帧">k{f.id}</KV><KV label="图像">{f.imageId === null ? "未冻结" : "#" + String(f.imageId).padStart(6, "0")}</KV><KV label="来源">{f.source === "history" ? f.sourceRecord : "相机采集"}</KV><KV label="原示教备份">{f.backup === null ? "—" : f.backup.imageId === null ? "原示教无图" : "#" + f.backup.imageId}</KV><button className="btn wf-full wf-gap-top" disabled={task.busy} onClick={() => { dispatch({ type: "restore-frame" }); notify("本帧已还原，请重新试匹配。"); }}><RotateCcw size={14} />{f.backup !== null ? "恢复原示教图" : "重置本帧参数"}</button></Panel><Notice title="保存与发布分别完成">本帧保存后，先验证代表性样本，再更新生产配方。</Notice></div></div>
  </>;
}

export function OverviewView() {
  const { state: s, dispatch, go, notify } = useWorkflow();
  const [editable, setEditable] = useState(false);
  const [error, setError] = useState("");
  const [reading, setReading] = useState(false);
  const file = useRef<HTMLInputElement>(null);
  const activeReader = useRef<FileReader | null>(null);
  const generation = useRef(0);
  const cancelRead = () => {
    generation.current++;
    const reader = activeReader.current;
    activeReader.current = null;
    if (reader) { reader.onload = null; reader.onerror = null; reader.onabort = null; if (reader.readyState === 1) reader.abort(); }
  };
  useEffect(() => () => { cancelRead(); }, []);
  const importImage = (selected: File | undefined) => {
    if (!selected) return;
    cancelRead(); setReading(false);
    if (!["image/png", "image/jpeg", "image/webp"].includes(selected.type) || selected.size > 1024 * 1024) { setError("请选择小于 1 MB 的 PNG、JPEG 或 WebP 图片。"); return; }
    const request = generation.current;
    const reader = new FileReader();
    activeReader.current = reader; setReading(true); setError("");
    const finish = () => { activeReader.current = null; setReading(false); };
    reader.onload = () => {
      if (request !== generation.current) return;
      finish();
      if (typeof reader.result !== "string" || !reader.result.startsWith("data:image/")) { setError("图像读取失败，请重试。"); return; }
      dispatch({ type: "overview", patch: { background: reader.result, saved: false } }); setError(""); notify("工件图已导入本地预览，仅用于总览显示。");
    };
    reader.onerror = () => { if (request !== generation.current) return; finish(); setError("图像读取失败，请重试。"); };
    reader.onabort = () => { if (request !== generation.current) return; finish(); };
    try { reader.readAsDataURL(selected); } catch { finish(); setError("图像读取失败，请重试。"); }
  };
  return <>
    <Steps active="overview" /><div className="wf-page-actions"><div className="wf-row"><Badge>显示布局</Badge><span className="wf-caption">独立于物理拍照坐标与毫米换算</span></div><button className="btn primary" disabled={reading} onClick={() => { dispatch({ type: "overview", patch: { saved: true } }); go("validation"); }}>保存布局并进入验证<ChevronRight size={15} /></button></div>
    <div className="wf-two-col wf-wide-left"><Panel title="工件总览" detail="点击帧框查看原图；开启布置后可拖动或用方向键调整" actions={<label className="wf-check"><input type="checkbox" checked={editable} onChange={e => setEditable(e.target.checked)} />调整显示框</label>}><OverviewMap editable={editable} /><div className="wf-actions"><input ref={file} type="file" accept="image/png,image/jpeg,image/webp" className="wf-hidden-input" aria-label="导入工件背景图" onChange={e => { const selected = e.target.files?.[0]; e.target.value = ""; importImage(selected); }} /><button className="btn" onClick={() => file.current?.click()}><Upload size={15} />导入工件图</button><button className="btn" onClick={() => { dispatch({ type: "overview", patch: { positions: initialPositions.map(p => ({ ...p })), saved: false } }); notify("已按几何规划恢复显示框。"); }}><RotateCcw size={15} />自动布置</button>{s.overview.background && <button className="btn" onClick={() => { cancelRead(); setReading(false); setError(""); dispatch({ type: "overview", patch: { background: null, saved: false } }); }}>恢复几何底图</button>}<button className="btn" disabled={reading} onClick={() => { dispatch({ type: "overview", patch: { saved: true } }); notify("总览显示布局已保存。"); }}><Save size={15} />保存布局</button></div>{reading && <span role="status">正在读取工件图…</span>}{error && <Notice title={error} tone="warn" />}</Panel><div className="stack"><Panel title={"选中 k" + s.selectedFrame} detail="总览与原图同步选择"><FrameCanvas id={s.selectedFrame} imageId={s.frames[s.selectedFrame - 1].imageId} label="本帧样本" /><div className="wf-actions"><button className="btn primary" onClick={() => go("teach")}><FileImage size={15} />进入本帧示教</button></div></Panel><Panel title="坐标与配置"><KV label="显示框中心">{s.overview.positions[s.selectedFrame - 1].x.toFixed(2)} / {s.overview.positions[s.selectedFrame - 1].y.toFixed(2)}</KV><KV label="物理视野">{s.recipe.fovWidth} × {s.recipe.fovHeight} mm</KV><KV label="物理覆盖">{coverage(s)}%</KV><KV label="布局状态"><Badge tone={s.overview.saved ? "ok" : "warn"}>{s.overview.saved ? "已保存" : "有未保存调整"}</Badge></KV><p className="wf-caption">总览用于工件定位和选帧，显示框调整不会改变测量结果。</p></Panel></div></div>
  </>;
}

export function PublishDialog({ onClose }: { onClose: () => void }) {
  const { state: s, dispatch, notify, go } = useWorkflow();
  const running = s.live.phase > 0 && s.live.phase < 4;
  return <Dialog title="确认发布候选配方" onClose={onClose}><div className="stack"><div className="wf-publish-versions"><div><span>当前生产</span><strong>v{s.recipe.production}</strong></div><ArrowRight size={20} /><div><span>已验证候选</span><strong className="wf-blue-text">v{s.recipe.candidate}</strong></div></div><KV label="产品">{s.recipe.name}</KV><KV label="验证记录">5 / 5 样本符合预期</KV><Notice title={running ? "当前工件完成后生效" : "将在下一个工件开始前生效"} tone={running ? "warn" : "info"}>{running ? "本件继续使用 v" + s.live.inFlightVersion + "，新版本排队等待。完整工件只使用一个配方版本。" : "更新后在线检测使用新版本。原始历史记录和原始判定保留。"}</Notice></div><div className="wf-dialog-actions"><button className="btn" onClick={onClose}>取消</button><button className="btn primary" disabled={!canPublish(s)} onClick={() => { dispatch({ type: "publish" }); notify(running ? "候选版本已排队，等待当前工件完成。" : "生产版本已更新为 v" + s.recipe.candidate + "。"); onClose(); if (running) go("live"); }}><ShieldCheck size={15} />确认发布</button></div></Dialog>;
}

export function ValidationView({ openConfirm = false }: { openConfirm?: boolean }) {
  const { state: s, go, dispatch } = useWorkflow();
  const [confirm, setConfirm] = useState(openConfirm);
  const task = useTask();
  const checks = validationChecks(s);
  const ready = checks.every(c => c.pass);
  const ran = s.validation.revision === s.recipe.revision && s.validation.status !== "idle";
  useEffect(() => { setConfirm(openConfirm); }, [openConfirm]);
  return <>
    {task.error && <Notice title="预览操作失败" tone="warn">{task.error}</Notice>}
    <Steps active="validation" /><div className="wf-page-actions"><div className="wf-row"><Badge tone="neutral">生产 v{s.recipe.production}</Badge><Badge>候选 v{s.recipe.candidate}</Badge>{s.live.queued !== null && <Badge tone="warn">v{s.live.queued} 等待本件完成</Badge>}</div><button className="btn primary" disabled={!canPublish(s)} onClick={() => setConfirm(true)}><ShieldCheck size={15} />发布候选配方</button></div>
    {!ready ? <Notice title="发布条件未满足" tone="warn">先完成下方检查项，再进行代表性样本验证。</Notice> : s.recipe.candidate === s.recipe.production ? <Notice title={"生产配方 v" + s.recipe.production + " 已生效"} tone="ok">可以进入在线检测。后续修改将创建新的候选版本。</Notice> : ran ? <Notice title={s.validation.status === "passed" ? "批量验证通过，可以发布候选配方" : "验证结果不符合样本预期"} tone={s.validation.status === "passed" ? "ok" : "warn"}>{s.validation.status === "passed" ? "5 / 5 代表性样本结果符合预期。修改配置后此验证会失效。" : "检查下方不一致样本，修正参数或示教后重新验证。"}</Notice> : <Notice title="配置检查通过，等待批量验证">使用正常、断胶、位置偏离、胶宽超限和测量异常样本验证。</Notice>}
    <div className="wf-two-col wf-wide-right"><Panel title="发布前检查" detail="点击未满足项，进入对应配置"><div className="wf-checklist">{checks.map(c => <button key={c.label} onClick={() => { if (c.view === "teach") { const missing = s.frames.find(f => !f.saved || !canSaveFrame(f)); if (missing) dispatch({ type: "select-frame", id: missing.id }); } go(c.view); }}><span className={c.pass ? "wf-check-circle done" : "wf-check-circle"}>{c.pass ? <Check size={14} /> : "!"}</span><div><strong>{c.label}</strong><small>{c.detail}</small></div><ChevronRight size={16} /></button>)}</div><div className="wf-gap-top"><KV label="标定">C{String(s.calibration.version).padStart(2, "0")}</KV><KV label="候选修订">{s.recipe.revision}</KV><KV label="发布时点">工件边界</KV></div></Panel>
    <Panel title="代表性样本验证" detail="样本预期与候选结果逐件对照" actions={<button className="btn primary" disabled={!ready || task.busy} onClick={() => task.run(() => dispatch({ type: "validate" }))}><BusyLabel busy={task.busy}><WandSparkles size={15} />运行批量验证</BusyLabel></button>}><div className="wf-table-wrap"><table className="table wf-table"><thead><tr><th>样本</th><th>预期</th><th>候选结果</th><th>检查</th></tr></thead><tbody>{validationSamples.map(sample => {
      const verdict = sampleVerdict(s, sample); const pass = verdict === sample.expected;
      return <tr key={sample.id}><td><strong>{sample.name}</strong><span className="wf-table-sub">{sample.id} · {sample.valid ? "原图可用" : "定位异常样本"}</span></td><td><VerdictBadge verdict={sample.expected} /></td><td>{ran ? <VerdictBadge verdict={verdict} /> : <span className="wf-caption">待验证</span>}</td><td>{ran ? <Badge tone={pass ? "ok" : "warn"}>{pass ? "一致" : "不一致"}</Badge> : "—"}</td></tr>;
    })}</tbody></table></div><div className="wf-stat-grid"><div><span>已验证样本</span><strong>{ran ? "5" : "0"} <small>/ 5</small></strong></div><div><span>符合预期</span><strong>{ran ? validationSamples.filter(sample => sampleVerdict(s, sample) === sample.expected).length : "—"}</strong></div><div><span>原始记录</span><strong>保留</strong></div></div><p className="wf-caption">配置、图像示教或标定变化后，需要重新验证当前候选修订。</p></Panel></div>
    <div className="wf-actions wf-end"><button className="btn" onClick={() => go("teach")}>返回示教</button><button className="btn" onClick={() => go("live")}>进入在线检测<ArrowRight size={15} /></button></div>
    {confirm && <PublishDialog onClose={() => setConfirm(false)} />}
  </>;
}
