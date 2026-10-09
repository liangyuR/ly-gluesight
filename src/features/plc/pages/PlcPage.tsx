import { useEffect, useMemo, useRef, useState } from "react";
import { FileJson, Pencil, PenLine, Plug, PlugZap, Plus, Save, Trash2, Undo2 } from "lucide-react";
import ConnectionForm, { connectionError } from "../components/ConnectionForm";
import PlcStatusBadge from "../components/PlcStatusBadge";
import PointEditor from "../components/PointEditor";
import PointsJsonDialog from "../components/PointsJsonDialog";
import WriteDialog from "../components/WriteDialog";
import S7Phase1Dialog from "../components/S7Phase1Dialog";
import S7RecipePlanPanel from "../components/S7RecipePlanPanel";
import { plcApi, usePlcOperationState, usePlcStatus, usePlcValues } from "../api";
import { inspectionHandshakeTags } from "../../../business/plcTags";
import { dataTypeLabels, edgeLabels, effectiveOrder, formatValue, isMultiWord, newPoint, protocols } from "../meta";
import { formatClock } from "../time";
import type { PlcConfig, PlcHandshakeState, PlcPoint, TagPreset } from "../types";

type Editing = { point: PlcPoint; isNew: boolean } | null;

interface PlcPageProps {
  tagPresets?: TagPreset[];
}

const configText = (config: PlcConfig | null) => JSON.stringify(config, (_key, value) =>
  typeof value === "number" && !Number.isFinite(value) ? String(value) : value);
const fieldsetStyle = { border: 0, padding: 0, margin: 0, minWidth: 0 };
const handshakePhaseLabels: Record<PlcHandshakeState["phase"], string> = {
  resetRequired: "等待 PLC 复位", idle: "空闲", validating: "校验请求", acquiring: "采集中",
  draining: "等待帧处理结束", awaitAck: "等待结果确认", releasing: "等待握手释放", fault: "故障",
};

export default function PlcPage({ tagPresets = [] }: PlcPageProps) {
  const status = usePlcStatus();
  const values = usePlcValues();
  const { blockedReason, handshake } = usePlcOperationState();
  const operationLock = useRef(blockedReason);
  operationLock.current = blockedReason;
  const linkState = useRef(status?.state);
  linkState.current = status?.state;
  const [saved, setSaved] = useState<PlcConfig | null>(null);
  const [draft, setDraft] = useState<PlcConfig | null>(null);
  const [editing, setEditing] = useState<Editing>(null);
  const [writing, setWriting] = useState<PlcPoint | null>(null);
  const [jsonOpen, setJsonOpen] = useState(false);
  const [s7Open, setS7Open] = useState(false);
  const [cpuPreset, setCpuPreset] = useState({ revision: 0, key: "" });
  const [notice, setNotice] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  const [loadError, setLoadError] = useState("");
  const [busy, setBusy] = useState(false);
  const acting = useRef(false);
  const loadSerial = useRef(0);
  const alive = useRef(true);

  const loadConfig = () => {
    const serial = ++loadSerial.current;
    setLoadError("");
    plcApi.getConfig().then((c) => {
      if (serial !== loadSerial.current) return;
      setSaved(c); setDraft(structuredClone(c));
    }).catch(e => { if (serial === loadSerial.current) setLoadError(String(e)); });
  };

  useEffect(() => {
    alive.current = true;
    loadConfig();
    return () => { alive.current = false; loadSerial.current++; };
  }, []);

  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => setNotice(null), 4000);
    return () => clearTimeout(t);
  }, [notice]);

  const dirty = useMemo(() => configText(saved) !== configText(draft), [saved, draft]);
  const connected = status?.state === "connected";
  const reconnectSaved = !!blockedReason && !!status && !connected;
  const tagLabel = useMemo(() => new Map(tagPresets.map((t) => [t.value, t.label])), [tagPresets]);

  if (!draft) return <div className="panel"><p role={loadError ? "alert" : "status"}>{loadError || "正在加载 PLC 配置…"}</p>{loadError && <button className="btn" onClick={loadConfig}>重新加载</button>}</div>;

  const canReconnect = () => !!linkState.current && linkState.current !== "connected";
  const perform = async (action: () => Promise<unknown>, allowReconnect = false) => {
    if (acting.current) return;
    if (operationLock.current && !(allowReconnect && canReconnect())) {
      setNotice({ kind: "error", text: operationLock.current }); return;
    }
    acting.current = true; setBusy(true); setNotice(null);
    try { await action(); }
    catch (e) { if (alive.current) setNotice({ kind: "error", text: String(e) }); }
    finally { acting.current = false; if (alive.current) setBusy(false); }
  };

  const invalid = connectionError(draft.connection)
    || (!Number.isSafeInteger(draft.heartbeat.intervalMs) || draft.heartbeat.intervalMs < 0 || (draft.heartbeat.pointId && draft.heartbeat.intervalMs < 100) ? "心跳周期需为整数，启用心跳时不能小于 100 ms" : "")
    || (!Number.isInteger(draft.logRetentionDays) || draft.logRetentionDays < 0 || draft.logRetentionDays > 0xffffffff ? "日志保留天数需为 0–4294967295 的整数" : "");

  const save = async () => {
    if (operationLock.current) { setNotice({ kind: "error", text: operationLock.current }); return false; }
    if (invalid) { setNotice({ kind: "error", text: invalid }); return false; }
    try {
      await plcApi.saveConfig(draft);
      if (!alive.current) return false;
      setSaved(structuredClone(draft));
      setNotice({ kind: "ok", text: "配置已保存" });
      return true;
    } catch (e) {
      if (alive.current) setNotice({ kind: "error", text: String(e) });
      return false;
    }
  };

  const toggleConnection = async () => {
    await perform(async () => {
      if (operationLock.current) {
        if (canReconnect()) await plcApi.connect();
      } else if (linkState.current === "connected") await plcApi.disconnect();
      else if (!invalid && (!dirty || (await save())) && alive.current && !operationLock.current && canReconnect()) await plcApi.connect();
    }, true);
  };

  const upsertPoint = (point: PlcPoint) => {
    if (operationLock.current) { setNotice({ kind: "error", text: operationLock.current }); return; }
    const points = editing?.isNew
      ? [...draft.points, point]
      : draft.points.map((p) => (p.id === editing?.point.id ? point : p));
    const heartbeat =
      !editing?.isNew && draft.heartbeat.pointId === editing?.point.id
        ? { ...draft.heartbeat, pointId: point.access === "readWrite" ? point.id : null }
        : draft.heartbeat;
    setDraft({ ...draft, points, heartbeat });
    setEditing(null);
  };

  const removePoint = (id: string) =>
    setDraft({
      ...draft,
      points: draft.points.filter((p) => p.id !== id),
      heartbeat: draft.heartbeat.pointId === id ? { ...draft.heartbeat, pointId: null } : draft.heartbeat,
    });

  const savedPoints = new Map(saved?.points.map((p) => [p.id, JSON.stringify(p)]));
  const meta = protocols[draft.connection.protocol];
  const writablePoints = draft.points.filter((p) => p.access === "readWrite");
  const protectedWrite = (point: PlcPoint) => saved?.connection.protocol === "s7"
    && (point.id === saved.heartbeat.pointId || point.tags.some(tag => inspectionHandshakeTags.has(tag)))
    ? "S7 握手和心跳由检测流程管理，不能手动写入" : "";

  return (
    <div className="stack plc">
      {blockedReason && <p className="notice error" role="status">{blockedReason}</p>}
      {handshake && <section className="panel" aria-label="S7 握手状态">
        <h3 className="panel-title">S7 握手状态</h3>
        <div className="row">
          <span>当前阶段：<b>{handshakePhaseLabels[handshake.phase]} <span className="mono">({handshake.phase})</span></b></span>
          <span>请求序号：<b className="mono">{handshake.requestSeq ?? "—"}</b></span>
          <span>结果序号：<b className="mono">{handshake.resultSeq ?? "—"}</b></span>
        </div>
        {handshake.message && <p className="muted hint">{handshake.message}</p>}
      </section>}
      <fieldset disabled={busy} className="stack" style={fieldsetStyle}>
      <div className="panel">
        <div className="panel-toolbar">
          <div className="row">
            <h3 className="panel-title">连接</h3>
            <PlcStatusBadge state={status?.state} />
            <span className="muted ellipsis" title={status?.message}>
              {status?.message}
            </span>
          </div>
          <div className="row">
            {dirty && <span className="badge warn">未保存</span>}
            <button className="btn" disabled={!dirty || !!blockedReason} onClick={() => saved && setDraft(structuredClone(saved))}>
              <Undo2 size={16} />
              还原
            </button>
            <button className="btn" disabled={!dirty || !!invalid || !!blockedReason} onClick={() => void perform(save)}>
              <Save size={16} />
              保存配置
            </button>
            <button className={`btn ${connected ? "danger" : "primary"}`} disabled={!status || (connected ? !!blockedReason : !reconnectSaved && !!invalid)} onClick={toggleConnection}>
              {connected ? <PlugZap size={16} /> : <Plug size={16} />}
              {connected ? "断开" : reconnectSaved ? "使用已保存配置重连" : status?.state === "error" || status?.state === "connecting" ? "重连" : "连接"}
            </button>
          </div>
        </div>

        {reconnectSaved && <p className="muted hint">重连使用后端已保存的配置，当前草稿不会保存或应用。</p>}
        <fieldset disabled={!!blockedReason} style={fieldsetStyle}>
        <ConnectionForm key={cpuPreset.revision} initialCpuPreset={cpuPreset.key} value={draft.connection} onChange={(connection) => setDraft({ ...draft, connection })} />

        <div className="form-grid section">
          <label className="field">
            <span>心跳点位</span>
            <select
              className="input"
              value={draft.heartbeat.pointId ?? ""}
              onChange={(e) => setDraft({ ...draft, heartbeat: { ...draft.heartbeat, pointId: e.target.value || null } })}
            >
              <option value="">不启用</option>
              {writablePoints.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}（{p.address}）
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>心跳周期 (ms)</span>
            <input
              className="input mono"
              type="number"
              min={100}
              step={1}
              disabled={!draft.heartbeat.pointId}
              value={Number.isFinite(draft.heartbeat.intervalMs) ? draft.heartbeat.intervalMs : ""}
              onChange={(e) => setDraft({ ...draft, heartbeat: { ...draft.heartbeat, intervalMs: e.target.value ? Number(e.target.value) : NaN } })}
            />
          </label>
          <label className="field">
            <span>日志保留天数（0 为永久）</span>
            <input
              className="input mono"
              type="number"
              min={0}
              step={1}
              value={Number.isFinite(draft.logRetentionDays) ? draft.logRetentionDays : ""}
              onChange={(e) => setDraft({ ...draft, logRetentionDays: e.target.value ? Number(e.target.value) : NaN })}
            />
          </label>
        </div>

        <div className="conn-footer">
          <label className="check">
            <input type="checkbox" checked={draft.autoConnect} onChange={(e) => setDraft({ ...draft, autoConnect: e.target.checked })} />
            <span>启动时自动连接</span>
          </label>
          <div className="stats-inline">
            <span>周期 <b className="mono">{status?.cycleMs ?? "--"} ms</b></span>
            <span>轮询 <b className="mono">{status?.pollCount ?? 0}</b></span>
            <span>错误 <b className="mono">{status?.errorCount ?? 0}</b></span>
            <span>最近 <b className="mono">{status?.lastPoll ? formatClock(status.lastPoll) : "--"}</b></span>
          </div>
        </div>
        </fieldset>
        {notice && <div className={`notice ${notice.kind}`}>{notice.text}</div>}
        {invalid && !connectionError(draft.connection) && <p role="alert" className="c-ng">{invalid}</p>}
      </div>

      <fieldset disabled={!!blockedReason} style={fieldsetStyle}>
      <div className="panel">
        <div className="panel-toolbar">
          <div className="row">
            <h3 className="panel-title">地址表</h3>
            <span className="muted">{draft.points.length} 个点位</span>
          </div>
          <div className="row">
            <button className="btn" onClick={() => setS7Open(true)}>载入一期 S7 点表</button>
            <button className="btn" onClick={() => setJsonOpen(true)}>
              <FileJson size={16} />
              导入/导出
            </button>
            <button className="btn primary" onClick={() => setEditing({ point: newPoint(), isNew: true })}>
              <Plus size={16} />
              新增点位
            </button>
          </div>
        </div>
        <p className="muted hint top">
          {meta.label} 地址示例：<span className="mono">{meta.examples.join("  ")}</span>
        </p>
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>名称</th>
                <th>地址</th>
                <th>类型</th>
                <th>读写</th>
                <th>边沿</th>
                <th>标签</th>
                <th>记录</th>
                <th>当前值</th>
                <th className="right">操作</th>
              </tr>
            </thead>
            <tbody>
              {draft.points.length === 0 && (
                <tr>
                  <td colSpan={9} className="muted center">
                    暂无点位，点击「新增点位」开始配置
                  </td>
                </tr>
              )}
              {draft.points.map((p) => {
                const v = values[p.id];
                const live = savedPoints.get(p.id) === JSON.stringify(p) && !dirty;
                return (
                  <tr key={p.id}>
                    <td>
                      <div className="cell-title">
                        {p.name}
                        {draft.heartbeat.pointId === p.id && <span className="tag subtle">心跳</span>}
                      </div>
                      {p.description && <div className="cell-sub">{p.description}</div>}
                    </td>
                    <td className="mono">{p.address}</td>
                    <td className="nowrap">
                      {dataTypeLabels[p.dataType].split(" ")[0]}
                      {isMultiWord(p.dataType) && <span className="cell-sub"> {effectiveOrder(p, draft.connection)}</span>}
                    </td>
                    <td>{p.access === "readWrite" ? "读写" : "只读"}</td>
                    <td>{p.edge === "none" ? <span className="muted">—</span> : edgeLabels[p.edge]}</td>
                    <td>
                      <div className="tags">
                        {p.tags.length === 0 && <span className="muted">—</span>}
                        {p.tags.map((t) => (
                          <span key={t} className="tag">
                            {tagLabel.get(t) ?? t}
                          </span>
                        ))}
                      </div>
                    </td>
                    <td>{p.logChanges ? "是" : <span className="muted">否</span>}</td>
                    <td className={`mono value ${live && status?.state === "connected" && v?.error ? "ng" : ""}`}>
                      {!live ? <span className="muted">待保存</span> : status?.state === "connected" ? formatValue(v) : <span className="muted">未连接</span>}
                    </td>
                    <td className="right nowrap">
                      <button
                        className="icon-btn"
                        title={protectedWrite(p) || "写入"}
                        aria-label="写入"
                        disabled={p.access !== "readWrite" || status?.state !== "connected" || !live || !!protectedWrite(p)}
                        onClick={() => setWriting(p)}
                      >
                        <PenLine size={16} />
                      </button>
                      <button className="icon-btn" title="编辑" onClick={() => setEditing({ point: p, isNew: false })}>
                        <Pencil size={16} />
                      </button>
                      <button className="icon-btn danger" title="删除" onClick={() => removePoint(p.id)}>
                        <Trash2 size={16} />
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </div>
      </fieldset>
      </fieldset>
      {draft.connection.protocol === "s7" && <S7RecipePlanPanel />}

      {editing && (
        <PointEditor
          initial={editing.point}
          isNew={editing.isNew}
          existingIds={draft.points.map((p) => p.id)}
          connection={draft.connection}
          tagPresets={tagPresets}
          blockedReason={blockedReason}
          onSave={upsertPoint}
          onClose={() => setEditing(null)}
        />
      )}
      {writing && (
        <WriteDialog
          point={writing}
          current={values[writing.id]}
          blockedReason={blockedReason || protectedWrite(writing)}
          onWrite={(value) => {
            const reason = operationLock.current || protectedWrite(writing);
            if (reason) return Promise.reject(new Error(reason));
            if (status?.state !== "connected" || dirty || savedPoints.get(writing.id) !== JSON.stringify(writing))
              return Promise.reject(new Error("连接状态或地址表已变化，请关闭后重新写入"));
            return plcApi.writePoint(writing.id, value);
          }}
          onClose={() => setWriting(null)}
        />
      )}
      {jsonOpen && (
        <PointsJsonDialog points={draft.points} blockedReason={blockedReason} onApply={(points) => {
          if (operationLock.current) return;
          setDraft({ ...draft, points,
          heartbeat: { ...draft.heartbeat, pointId: points.some(p => p.id === draft.heartbeat.pointId && p.access === "readWrite") ? draft.heartbeat.pointId : null },
        }); }} onClose={() => setJsonOpen(false)} />
      )}
      {s7Open && <S7Phase1Dialog config={draft} dirty={dirty} blockedReason={blockedReason} onClose={() => setS7Open(false)} onApply={(config, cpu) => {
        if (operationLock.current) return;
        setDraft(config);
        setCpuPreset(previous => ({ revision: previous.revision + 1, key: cpu }));
        setS7Open(false);
        setNotice({ kind: "ok", text: "一期 S7 点表已载入草稿，请核对地址后保存配置" });
      }} />}
    </div>
  );
}
