import { useState } from "react";
import { ArrowLeft, ChevronRight, Download, FileImage, FilterX, Play, ScanEye, Square, WandSparkles } from "lucide-react";
import { useWorkflow } from "./context";
import { cameraFor, historyRecords, layoutCompatible } from "./model";
import { Badge, BusyLabel, Curve, FrameCanvas, FrameList, KV, Notice, OverviewMap, Panel, SelectField, VerdictBadge, useTask } from "./components";

export function LiveView() {
  const { state: s, dispatch, go } = useWorkflow();
  const l = s.live;
  const config = l.inFlightConfig ?? s.productionConfig;
  const running = l.phase > 0 && l.phase < 4;
  const ready = s.recipe.production > 0 && s.productionConfig.shots.every(f => { const c = cameraFor(s, f.camera); return c?.connected && c.applied && f.view <= c.viewCount; }) && s.plc.connected && s.plc.ready;
  const ng = l.result === "NG", err = l.result === "ERR";
  const acquired = l.phase >= 3 || (l.phase === 2 && s.selectedFrame < 4);
  const rawMissing = err && s.selectedFrame === 3;
  const verdictText = l.result ?? (running ? "检测中" : "等待工件");
  const status = l.phase === 4 ? l.result === "OK" ? "胶路合格" : ng ? "断胶超限 · 6.2 mm" : "k3 原图缺失" : ["设备就绪，等待开始信号", "本件相机布防", "正在接收触发帧", "计算与汇总本件结果"][l.phase];
  return <>
    <div className="wf-page-actions"><div className="wf-row"><Badge tone={ready ? "ok" : "warn"}>{ready ? "设备与握手就绪" : "设备或握手未就绪"}</Badge><Badge tone="neutral">生产 v{s.recipe.production}</Badge><span className="wf-caption">飞拍 · {config.recipe.name}</span></div><div className="wf-row"><button className="btn" disabled={!l.accepting} onClick={() => dispatch({ type: "live-stop" })}><Square size={14} />{running ? "本件后停止" : "停止接件"}</button><button className="btn primary" disabled={!ready || running} onClick={() => dispatch({ type: "live-start" })}><Play size={15} />{l.phase === 4 ? "开始下一件" : "启动检测"}</button></div></div>
    {!ready && <Notice title="启动条件未满足" tone="warn">请确认相机参数已接受、PLC 业务握手就绪，并存在有效生产版本。</Notice>}
    {running && !l.accepting && <Notice title="已停止接收新工件，当前工件继续完成" tone="warn">当前工件使用 v{l.inFlightVersion}，完成后保留本件结果。</Notice>}
    {l.queued !== null && <Notice title={"候选 v" + l.queued + " 已排队，等待本件结束"}>本件仍使用 v{l.inFlightVersion}，完整工件使用同一个配方版本。</Notice>}
    <div className="wf-live-grid"><div className="stack"><div className="wf-two-col wf-live-images"><Panel title="整件总览" detail="点击帧框定位到原图" actions={<Badge tone="neutral">TJ-{String(l.part).padStart(6, "0")}</Badge>}><OverviewMap defect={ng} missing={err} shots={config.shots} overview={config.overview} /></Panel><Panel title={"k" + s.selectedFrame + " · 原图"} detail={rawMissing ? "本帧测量异常" : l.phase === 4 ? "本帧结果与整件结论分别显示" : "采集中按帧查看"} actions={<Badge tone={rawMissing ? "warn" : ng && s.selectedFrame === 3 ? "ng" : acquired ? "ok" : "neutral"}>{rawMissing ? "ERR" : ng && s.selectedFrame === 3 ? "NG · 断胶" : acquired ? "本帧 OK" : "等待图像"}</Badge>}><FrameCanvas id={s.selectedFrame} imageId={acquired ? l.part * 10 + s.selectedFrame : null} shot={config.shots[s.selectedFrame - 1]} overlay={l.phase >= 3} defect={ng} missing={rawMissing} label="采集原图" /></Panel></div>
    <FrameList frames={config.shots} selected={s.selectedFrame} horizontal onSelect={id => dispatch({ type: "select-frame", id })} defect={ng ? 3 : undefined} missing={err ? 3 : undefined} labels={config.shots.map(f => l.phase === 4 ? err && f.id === 3 ? "ERR · 原图缺失" : ng && f.id === 3 ? "NG · 断胶" : "本帧 OK" : l.phase >= 3 ? "测量中" : l.phase === 2 && f.id < 4 ? "已采集" : "等待图像")} />
    <Panel title={config.shots[s.selectedFrame - 1].shotId + " · 本点测量曲线"} detail="本点内弧长；拍照点之间分别统计，整件结论保持不变"><Curve defect={ng && s.selectedFrame === 3} invalid={err && s.selectedFrame === 3} recipe={config.recipe} /></Panel>
    {ng && <div className="wf-defect-row"><Badge tone="ng">断胶</Badge><div><strong>52.4–58.6 mm · 长度 6.2 mm</strong><span className="wf-caption">P3 点内弧长 · 允许 {config.shots[2].params.gapLimit ?? config.recipe.maxGap} mm</span></div><button className="btn" onClick={() => dispatch({ type: "select-frame", id: 3 })}>定位缺陷帧</button><button className="btn" onClick={() => { dispatch({ type: "record", id: "TJ-000184" }); go("record"); }}>历史复盘<ChevronRight size={14} /></button></div>}
    </div><div className="stack"><Panel title="整件判定"><div className={"wf-result-card " + (ng ? "is-ng" : err ? "is-warn" : l.result === "OK" ? "is-ok" : "")}><span>整件结果</span><strong>{verdictText}</strong><p>{status}</p></div><KV label="cycleId">{l.cycleId ?? "待开始"}</KV><KV label="本件示例发布包">{config.bundleId}</KV><KV label="本件版本">{l.inFlightVersion === null ? "待开始" : "v" + l.inFlightVersion}</KV><KV label="实际图像">{l.phase === 4 ? (err ? "5 / 6" : "6 / 6") : l.phase === 3 ? "6 / 6" : l.phase === 2 ? "3 / 6" : "0 / 6"}</KV><KV label="示例整件耗时">{l.phase === 4 ? "1.08 s" : "—"}</KV>{err && <Notice title="测量数据无效" tone="warn">原图缺失导致 ERR。先排查采集链路，再决定工件处置。</Notice>}</Panel>
    <Panel title="工件事件" detail="从开始到结果确认"><ol className="wf-cycle-events">{["开始信号", "相机布防", "接收触发帧", "测量汇总", "输出结果", "PLC 确认"].map((p, i) => <li key={p} className={l.phase === 4 || (l.phase > 0 && i <= l.phase) ? "done" : ""}><span /><div><strong>{p}</strong><small>{l.phase === 4 ? (i < 4 ? "已完成" : l.result) : i <= l.phase && l.phase > 0 ? "已完成 / 进行中" : "等待"}</small></div></li>)}</ol></Panel>
    <Panel title="运行情景" detail="用于体验不同的检测结果"><label className="wf-check"><input type="checkbox" checked={l.continuous} onChange={e => dispatch({ type: "live-option", patch: { continuous: e.target.checked } })} />连续预览</label><SelectField label="下一件样本" value={l.scenario === "OK" ? "正常胶路 · OK" : l.scenario === "NG" ? "断胶超限 · NG" : "原图缺失 · ERR"} options={["正常胶路 · OK", "断胶超限 · NG", "原图缺失 · ERR"]} disabled={running} onChange={v => dispatch({ type: "live-option", patch: { scenario: v.includes("NG") ? "NG" : v.includes("ERR") ? "ERR" : "OK" } })} />{running && <div className="wf-actions"><label className="wf-check"><input type="checkbox" checked={l.auto} onChange={e => dispatch({ type: "live-option", patch: { auto: e.target.checked } })} />自动演示事件</label><button className="btn" onClick={() => dispatch({ type: "live-step" })}>推进一次预览事件</button></div>}</Panel></div></div>
  </>;
}

export function HistoryView() {
  const { state: s, dispatch, go, notify } = useWorkflow();
  const [sn, setSn] = useState("");
  const [result, setResult] = useState("全部结果");
  const [raw, setRaw] = useState("全部原图状态");
  const [start, setStart] = useState("");
  const [end, setEnd] = useState("");
  const [exportError, setExportError] = useState("");
  const invalidDates = Boolean(start && end && start > end);
  const records = historyRecords.filter(r => r.id.toLowerCase().includes(sn.toLowerCase()) && (result === "全部结果" || r.result === result) && (raw === "全部原图状态" || r.raw === (raw === "原图可用")) && (!start || r.time.slice(0, 10) >= start) && (!end || r.time.slice(0, 10) <= end));
  const exportRows = () => {
    const text = "\uFEFF工件SN,cycleId,发布包,时间,结果,原因,原图,版本\n" + records.map(r => [r.id, r.cycleId, r.bundleId, r.time, r.result, r.cause, r.raw ? "可用" : "已清理", "v" + r.version].join(",")).join("\n");
    let url: string | null = null;
    try {
      url = URL.createObjectURL(new Blob([text], { type: "text/csv;charset=utf-8" }));
      const a = document.createElement("a"); a.href = url; a.download = "涂胶历史记录-示例.csv"; a.click(); setExportError(""); notify("当前筛选记录已导出。");
    } catch { setExportError("导出失败，请重试。"); }
    finally { if (url) { const exportedUrl = url; setTimeout(() => URL.revokeObjectURL(exportedUrl), 1000); } }
  };
  return <>
    <div className="wf-page-actions"><span className="wf-caption">{records.length} 件记录 · 原始检测结果</span><button className="btn" disabled={!records.length || invalidDates} onClick={exportRows}><Download size={15} />导出当前筛选</button></div>
    {exportError && <Notice title={exportError} tone="warn" />}
    {invalidDates && <Notice title="开始日期不能晚于结束日期" tone="warn" />}
    <Panel title="筛选工件"><div className="wf-history-filters"><label className="wf-field"><span>工件 SN</span><input className="input" placeholder="输入完整或部分 SN" value={sn} onChange={e => setSn(e.target.value)} /></label><label className="wf-field"><span>开始日期</span><input className="input" type="date" value={start} onChange={e => setStart(e.target.value)} /></label><label className="wf-field"><span>结束日期</span><input className="input" type="date" value={end} min={start} onChange={e => setEnd(e.target.value)} /></label><SelectField label="检测结果" value={result} options={["全部结果", "OK", "NG", "ERR"]} onChange={setResult} /><SelectField label="原图状态" value={raw} options={["全部原图状态", "原图可用", "原图已清理"]} onChange={setRaw} /><button className="btn" onClick={() => { setSn(""); setResult("全部结果"); setRaw("全部原图状态"); setStart(""); setEnd(""); }}><FilterX size={15} />清空</button></div></Panel>
    <Panel title="历史记录" detail="每条记录保留 cycleId 与发布包身份，可原包重现或按当前候选复测"><div className="wf-table-wrap"><table className="table wf-table"><thead><tr><th>工件 SN / cycleId</th><th>原发布包</th><th>检测时间</th><th>原始结果</th><th>原因</th><th>原图</th><th>配方</th><th>操作</th></tr></thead><tbody>{records.map(r => <tr key={r.id}><td><strong>{r.id}</strong><span className="wf-table-sub">{r.cycleId}</span></td><td className="wf-resource-id">{r.bundleId}</td><td>{r.time}</td><td><VerdictBadge verdict={r.result} /></td><td>{r.cause}</td><td><Badge tone={r.raw ? "neutral" : "warn"}>{r.raw ? r.frames + " 帧可用" : "已清理"}</Badge></td><td>v{r.version}</td><td><button className="btn" onClick={() => { dispatch({ type: "record", id: r.id }); dispatch({ type: "select-frame", id: 3 }); go("record"); }}>查看记录<ChevronRight size={14} /></button></td></tr>)}</tbody></table>{!records.length && <div className="empty small"><ScanEye size={30} /><strong>没有匹配的历史记录</strong><span>调整时间、SN 或结果筛选</span></div>}</div></Panel>
    <div className="wf-two-col"><Notice title="原始记录独立保留">按原发布包重现使用原点位资源与规则；按当前候选复测使用当前像素中线和参数。两者均生成独立示例对照，原始记录保持可回放。</Notice><Notice title="原图与测量数据分别留存" tone="info">仅保留测量数据的工件可重新判定，原图复测和示教取样需要可用原图。</Notice></div><div className="wf-caption">当前候选 v{s.recipe.candidate} · 支持按拍照点设备与视角载入历史样本</div>
  </>;
}

export function RecordView() {
  const { state: s, dispatch, go, notify } = useWorkflow();
  const task = useTask(), r = historyRecords.find(r => r.id === s.record) ?? historyRecords[0];
  const comparison = s.comparisons[r.id], shot = r.config.shots[s.selectedFrame - 1], candidate = s.frames[s.selectedFrame - 1];
  const missing = !r.raw || r.result === "ERR" && s.selectedFrame === 3;
  const sameSource = shot.camera === candidate.camera && shot.view === candidate.view && shot.shotId === candidate.shotId && shot.poseId === candidate.poseId;
  const compatible = layoutCompatible(s, r.config);
  const sameImages = s.frames.every((f, i) => f.camera === r.config.shots[i].camera && f.view === r.config.shots[i].view && f.poseId === r.config.shots[i].poseId && f.shotId === r.config.shots[i].shotId);
  const compare = (kind: "remeasure" | "rejudge", mode: "original" | "candidate") => task.run(() => { dispatch({ type: "compare", kind, mode }); notify("示例对照已生成，原始记录与原图保留；此操作未调用真实 DLL。"); });
  return <>
    {task.error && <Notice title="预览操作失败" tone="warn">{task.error}</Notice>}
    <div className="wf-page-actions"><div className="wf-row"><button className="btn" onClick={() => go("history")}><ArrowLeft size={15} />返回记录</button><Badge tone="neutral">{r.id}</Badge><span className="wf-caption">{r.cycleId} · {r.time}</span></div><button className="btn primary" disabled={missing || !sameSource || task.busy} onClick={() => { dispatch({ type: "capture", history: true }); go("teach"); notify("历史 " + shot.shotId + " / " + shot.camera + " / 视角 " + shot.view + " 已载入示教，原示教已备份。"); }}><FileImage size={15} />用作本点示教样本</button></div>
    {!r.raw && <Notice title="原图已按留存策略清理" tone="warn">可以回放测量数据，布局兼容时按候选规则重判。原包重现、候选原图复测与示教取样暂不可用。</Notice>}
    {!sameSource && <Notice title="当前点位设备或视角与历史不一致" tone="warn">不能将此历史原图绑定到当前拍照点。先核对 ID、Pose、camera 和 view。</Notice>}
    <div className="wf-comparison-grid"><Panel title="原始检测" detail="示例原始记录 · 独立保留" className="wf-original-record">
      <div className="wf-row"><VerdictBadge verdict={r.result} /><strong>生产配方 v{r.version}</strong><span className="wf-caption">{r.cause}</span></div>
      <KV label="cycleId">{r.cycleId}</KV><KV label="原示例发布包">{r.bundleId}</KV><KV label="原配方身份">{r.config.recipeRevision}</KV><KV label="算法图 / 引擎">{r.config.graphVersion} / {r.config.engineVersion}</KV>
      <div className="wf-stat-grid"><div><span>原始断胶长度</span><strong>{r.result === "ERR" ? "无效" : r.gap.toFixed(1) + " mm"}</strong></div><div><span>原图</span><strong>{r.raw ? r.frames + " 帧" : "已清理"}</strong></div><div><span>原始判定</span><strong>保留</strong></div></div>
    </Panel><Panel title="复测对照" detail="按原发布包重现 / 按当前候选复测">
      {comparison ? <><div className="wf-row"><VerdictBadge verdict={comparison.verdict} /><strong>{comparison.mode === "original" ? "原发布包 v" : "候选 v"}{comparison.version}</strong><Badge tone="neutral">{comparison.kind === "rejudge" ? "候选规则重判" : comparison.mode === "original" ? "原包重现" : "候选复测"}</Badge></div><KV label="对照 cycleId">{comparison.cycleId}</KV><KV label="对照资源身份">{comparison.bundleId}</KV><div className="wf-stat-grid"><div><span>对照断胶长度</span><strong>{comparison.verdict === "ERR" ? "无效" : comparison.gap.toFixed(1) + " mm"}</strong></div><div><span>与原始结论</span><strong>{comparison.verdict === r.result ? "一致" : "有变化"}</strong></div><div><span>原始记录</span><strong>保留</strong></div></div></> : <p className="wf-caption wf-empty-inline">原包重现冻结原中线、比例、测点、图与规则；候选复测使用当前候选。结果均为示例。</p>}
      <div className="wf-actions"><button className="btn" disabled={!r.raw || task.busy} onClick={() => compare("remeasure", "original")}><WandSparkles size={15} />按原发布包重现</button><button className="btn primary" disabled={!r.raw || !sameImages || task.busy} onClick={() => compare("remeasure", "candidate")}><BusyLabel busy={task.busy}>按当前候选复测</BusyLabel></button><button className="btn" disabled={!compatible || task.busy} onClick={() => compare("rejudge", "candidate")}>按候选规则重判</button></div>
      {!compatible && <Notice title="测点布局不兼容，不能按候选规则重判" tone="warn">中线、比例、站距或点位身份变化后，应在对应设备视角的历史原图上重新测量。</Notice>}
      <Notice title="示例对照不代表算法验证">本原型用固定样本演示资源选择与判定差异，未加载真实 DLL；复测不会自动消除原始断胶。</Notice>
    </Panel></div>
    <div className="wf-two-col"><Panel title="原始工件总览" detail="点击拍照点查看原设备视角的图像">
      <OverviewMap defect={r.result === "NG"} missing={r.result === "ERR"} shots={r.config.shots} overview={r.config.overview} />
      <FrameList frames={r.config.shots} selected={s.selectedFrame} horizontal onSelect={id => dispatch({ type: "select-frame", id })} defect={r.result === "NG" ? 3 : undefined} missing={r.result === "ERR" ? 3 : undefined} labels={r.config.shots.map(f => !r.raw ? "原图已清理" : r.result === "ERR" && f.id === 3 ? "原图缺失" : r.result === "NG" && f.id === 3 ? "NG · 断胶" : "原图可用")} />
    </Panel><Panel title={shot.shotId + " · 历史原图"} detail={shot.camera + " / 视角 " + shot.view + " · " + shot.poseId + " · " + shot.mmPerPx + " mm/px"}>
      <FrameCanvas id={s.selectedFrame} imageId={r.raw ? Number(r.id.replace("TJ-", "")) * 1000 + s.selectedFrame : null} shot={shot} overlay defect={r.result === "NG"} missing={missing} label="历史原图" /><div className="wf-gap-top"><KV label="图像来源">{r.cycleId} / {shot.shotId} / {shot.camera} / view {shot.view}</KV><KV label="原始配方">v{r.version}</KV></div>
    </Panel></div>
    <Panel title="原始测量数据" detail="各拍照点分别量测；曲线示例保留原始含义"><Curve defect={r.result === "NG" && s.selectedFrame === 3} invalid={r.result === "ERR" && s.selectedFrame === 3} recipe={r.config.recipe} /></Panel>
  </>;
}
