import { useState } from "react";
import { ArrowLeft, ChevronRight, Download, FileImage, FilterX, Play, ScanEye, Square, WandSparkles } from "lucide-react";
import { useWorkflow } from "./context";
import { defaultRecipe, historyRecords, initialPositions } from "./model";
import { Badge, BusyLabel, Curve, FrameCanvas, FrameList, KV, Notice, OverviewMap, Panel, SelectField, VerdictBadge, useTask } from "./components";

export function LiveView() {
  const { state: s, dispatch, go } = useWorkflow();
  const l = s.live;
  const config = l.inFlightConfig ?? s.productionConfig;
  const running = l.phase > 0 && l.phase < 4;
  const ready = s.recipe.production > 0 && s.device.connected && s.device.applied && s.plc.connected && s.plc.ready;
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
    <div className="wf-live-grid"><div className="stack"><div className="wf-two-col wf-live-images"><Panel title="整件总览" detail="点击帧框定位到原图" actions={<Badge tone="neutral">TJ-{String(l.part).padStart(6, "0")}</Badge>}><OverviewMap defect={ng} missing={err} recipe={config.recipe} overview={config.overview} /></Panel><Panel title={"k" + s.selectedFrame + " · 原图"} detail={rawMissing ? "本帧测量异常" : l.phase === 4 ? "本帧结果与整件结论分别显示" : "采集中按帧查看"} actions={<Badge tone={rawMissing ? "warn" : ng && s.selectedFrame === 3 ? "ng" : acquired ? "ok" : "neutral"}>{rawMissing ? "ERR" : ng && s.selectedFrame === 3 ? "NG · 断胶" : acquired ? "本帧 OK" : "等待图像"}</Badge>}><FrameCanvas id={s.selectedFrame} imageId={acquired ? l.part * 10 + s.selectedFrame : null} overlay={l.phase >= 3} defect={ng} missing={rawMissing} label="采集原图" /></Panel></div>
    <FrameList frames={s.frames} selected={s.selectedFrame} horizontal onSelect={id => dispatch({ type: "select-frame", id })} defect={ng ? 3 : undefined} missing={err ? 3 : undefined} labels={s.frames.map(f => l.phase === 4 ? err && f.id === 3 ? "ERR · 原图缺失" : ng && f.id === 3 ? "NG · 断胶" : "本帧 OK" : l.phase >= 3 ? "测量中" : l.phase === 2 && f.id < 4 ? "已采集" : "等待图像")} />
    <Panel title="整件测量曲线" detail="保持整件范围，选帧不会改变整件判定"><Curve defect={ng} invalid={err} recipe={config.recipe} /></Panel>
    {ng && <div className="wf-defect-row"><Badge tone="ng">断胶</Badge><div><strong>352.4–358.6 mm · 长度 6.2 mm</strong><span className="wf-caption">位置关联 k3 · 允许 {config.recipe.maxGap} mm</span></div><button className="btn" onClick={() => dispatch({ type: "select-frame", id: 3 })}>定位缺陷帧</button><button className="btn" onClick={() => { dispatch({ type: "record", id: "TJ-000184" }); go("record"); }}>历史复盘<ChevronRight size={14} /></button></div>}
    </div><div className="stack"><Panel title="整件判定"><div className={"wf-result-card " + (ng ? "is-ng" : err ? "is-warn" : l.result === "OK" ? "is-ok" : "")}><span>整件结果</span><strong>{verdictText}</strong><p>{status}</p></div><KV label="本件版本">{l.inFlightVersion === null ? "待开始" : "v" + l.inFlightVersion}</KV><KV label="实际图像">{l.phase === 4 ? (err ? "5 / 6" : "6 / 6") : l.phase === 3 ? "6 / 6" : l.phase === 2 ? "3 / 6" : "0 / 6"}</KV><KV label="示例整件耗时">{l.phase === 4 ? "1.08 s" : "—"}</KV>{err && <Notice title="测量数据无效" tone="warn">原图缺失导致 ERR。先排查采集链路，再决定工件处置。</Notice>}</Panel>
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
    const text = "\uFEFF工件SN,时间,结果,原因,原图,版本\n" + records.map(r => [r.id, r.time, r.result, r.cause, r.raw ? "可用" : "已清理", "v" + r.version].join(",")).join("\n");
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
    <Panel title="历史记录" detail="进入详情后可回放帧图像并对照候选配置"><div className="wf-table-wrap"><table className="table wf-table"><thead><tr><th>工件 SN</th><th>检测时间</th><th>原始结果</th><th>原因</th><th>原图</th><th>配方</th><th>操作</th></tr></thead><tbody>{records.map(r => <tr key={r.id}><td><strong>{r.id}</strong><span className="wf-table-sub">工件 A · 飞拍</span></td><td>{r.time}</td><td><VerdictBadge verdict={r.result} /></td><td>{r.cause}</td><td><Badge tone={r.raw ? "neutral" : "warn"}>{r.raw ? r.frames + " 帧可用" : "已清理"}</Badge></td><td>v{r.version}</td><td><button className="btn" onClick={() => { dispatch({ type: "record", id: r.id }); dispatch({ type: "select-frame", id: 3 }); go("record"); }}>查看记录<ChevronRight size={14} /></button></td></tr>)}</tbody></table>{!records.length && <div className="empty small"><ScanEye size={30} /><strong>没有匹配的历史记录</strong><span>调整时间、SN 或结果筛选</span></div>}</div></Panel>
    <div className="wf-two-col"><Notice title="原始记录独立保留">候选配置的重判或原图复测会生成对照结果，原始配方 v13 和原始判定保持可回放。</Notice><Notice title="原图与测量数据分别留存" tone="info">仅保留测量数据的工件可重新判定，原图复测和示教取样需要可用原图。</Notice></div><div className="wf-caption">当前候选 v{s.recipe.candidate} · 支持以历史样本进入单帧示教</div>
  </>;
}

export function RecordView() {
  const { state: s, dispatch, go, notify } = useWorkflow();
  const task = useTask();
  const r = historyRecords.find(r => r.id === s.record) ?? historyRecords[0];
  const comparison = s.comparisons[r.id];
  const missing = !r.raw || (r.result === "ERR" && s.selectedFrame === 3);
  const sampleAvailable = !missing;
  const compare = (kind: "remeasure" | "rejudge") => task.run(() => { dispatch({ type: "compare", kind }); notify(kind === "remeasure" ? "候选原图复测已完成，原始记录保留。" : "候选规则重判已完成，原始测量数据保留。"); });
  return <>
    {task.error && <Notice title="预览操作失败" tone="warn">{task.error}</Notice>}
    <div className="wf-page-actions"><div className="wf-row"><button className="btn" onClick={() => go("history")}><ArrowLeft size={15} />返回记录</button><Badge tone="neutral">{r.id}</Badge><span className="wf-caption">{r.time}</span></div><button className="btn primary" disabled={!sampleAvailable || task.busy} onClick={() => { dispatch({ type: "capture", history: true }); go("teach"); notify("历史 k" + s.selectedFrame + " 已载入示教，原示教图已备份。"); }}><FileImage size={15} />用作本帧示教样本</button></div>
    {!r.raw && <Notice title="原图已按留存策略清理" tone="warn">可以回放测量数据、按候选规则重新判定。原图复测和示教取样暂不可用。</Notice>}
    <div className="wf-comparison-grid"><Panel title="原始检测" detail="原始记录 · 独立保留" className="wf-original-record"><div className="wf-row"><VerdictBadge verdict={r.result} /><strong>生产配方 v{r.version}</strong><span className="wf-caption">{r.cause}</span></div><div className="wf-stat-grid"><div><span>原始断胶长度</span><strong>{r.result === "ERR" ? "无效" : r.gap.toFixed(1) + " mm"}</strong></div><div><span>原图</span><strong>{r.raw ? r.frames + " 帧" : "已清理"}</strong></div><div><span>原始判定</span><strong>保留</strong></div></div></Panel><Panel title="候选对照" detail={"候选 v" + s.recipe.candidate + " · 重判 / 原图复测"}>{comparison ? <><div className="wf-row"><VerdictBadge verdict={comparison.verdict} /><strong>候选 v{comparison.version}</strong><Badge tone="neutral">{comparison.kind === "remeasure" ? "原图复测" : "规则重判"}</Badge></div><div className="wf-stat-grid"><div><span>候选断胶长度</span><strong>{comparison.verdict === "ERR" ? "无效" : comparison.gap.toFixed(1) + " mm"}</strong></div><div><span>与原始结论</span><strong>{comparison.verdict === r.result ? "一致" : "有变化"}</strong></div><div><span>原始记录</span><strong>保留</strong></div></div></> : <p className="wf-caption wf-empty-inline">选择重新判定，或用原图重新测量，再查看候选结果。</p>}<div className="wf-actions"><button className="btn" disabled={task.busy} onClick={() => compare("rejudge")}><BusyLabel busy={task.busy}>按候选规则重判</BusyLabel></button><button className="btn primary" disabled={!r.raw || task.busy} onClick={() => compare("remeasure")}><WandSparkles size={15} />用原图重新测量</button></div></Panel></div>
    <div className="wf-two-col"><Panel title="原始工件总览" detail="点击帧框查看对应原图"><OverviewMap defect={r.result === "NG"} missing={r.result === "ERR"} recipe={defaultRecipe} overview={{ positions: initialPositions, background: null, saved: true }} /><FrameList frames={s.frames} selected={s.selectedFrame} horizontal onSelect={id => dispatch({ type: "select-frame", id })} defect={r.result === "NG" ? 3 : undefined} missing={r.result === "ERR" ? 3 : undefined} labels={s.frames.map(f => !r.raw ? "原图已清理" : r.result === "ERR" && f.id === 3 ? "原图缺失" : r.result === "NG" && f.id === 3 ? "NG · 断胶" : "原图可用")} /></Panel><Panel title={"k" + s.selectedFrame + " · 历史原图"} detail={r.raw ? "原始采集条件 · 曝光 60 μs · 标定 C07" : "原图不可用，测量数据仍保留"}><FrameCanvas id={s.selectedFrame} imageId={r.raw ? Number(r.id.replace("TJ-", "")) * 1000 + s.selectedFrame : null} overlay defect={r.result === "NG"} missing={missing} label="历史原图" /><div className="wf-gap-top"><KV label="图像来源">{r.id} / k{s.selectedFrame}</KV><KV label="原始配方">v{r.version}</KV></div></Panel></div>
    <Panel title="原始测量数据" detail="曲线与缺陷区间保留原始含义"><Curve defect={r.result === "NG"} invalid={r.result === "ERR"} recipe={defaultRecipe} /></Panel>
  </>;
}
