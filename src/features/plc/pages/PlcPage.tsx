import { useEffect, useMemo, useRef, useState } from "react";
import { FileJson, Pencil, PenLine, Plug, PlugZap, Plus, RotateCcw, Save, Trash2, Undo2 } from "lucide-react";
import { Alert, Button, Card, Checkbox, Chip, EmptyState, Heading, IconButton, NumberField, Select, Stack, StatusBadge, Table, Text, type TableColumn } from "qing-steel";
import ConnectionForm, { connectionError } from "../components/ConnectionForm";
import PlcStatusBadge from "../components/PlcStatusBadge";
import PointEditor from "../components/PointEditor";
import PointsJsonDialog from "../components/PointsJsonDialog";
import WriteDialog from "../components/WriteDialog";
import S7Phase1Dialog from "../components/S7Phase1Dialog";
import S7RecipePlanPanel from "../components/S7RecipePlanPanel";
import { plcApi, usePlcOperationState, usePlcStatus, usePlcValues } from "../api";
import { inspectionHandshakeTags } from "../../../business/plcTags";
import { asS7, dataTypeLabels, edgeLabels, effectiveOrder, formatValue, isMultiWord, newPoint, s7Protocol } from "../meta";
import { formatClock } from "../time";
import { useSnackbar } from "qing-steel";
import type { PlcConfig, PlcHandshakeState, PlcPoint, TagPreset } from "../types";
import { cycleApi } from "../../cycle/api";

type Editing = { point: PlcPoint; isNew: boolean } | null;

interface PlcPageProps {
  tagPresets?: TagPreset[];
  readOnly?: boolean;
}

const configText = (config: PlcConfig | null) => JSON.stringify(config, (_key, value) =>
  typeof value === "number" && !Number.isFinite(value) ? String(value) : value);
const fieldsetStyle = { border: 0, padding: 0, margin: 0, minWidth: 0 };
const handshakePhaseLabels: Record<PlcHandshakeState["phase"], string> = {
  resetRequired: "待复位", idle: "空闲", validating: "校验请求", acquiring: "采集中",
  draining: "等待帧处理结束", awaitAck: "等待结果确认", releasing: "等待握手释放", fault: "故障",
};

export default function PlcPage({ tagPresets = [], readOnly = false }: PlcPageProps) {
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
  const [planActionsTarget, setPlanActionsTarget] = useState<HTMLDivElement | null>(null);
  const [actionError, setActionError] = useState("");
  const { show: snack } = useSnackbar();
  const [loadError, setLoadError] = useState("");
  const [busy, setBusy] = useState(false);
  const [resetPending, setResetPending] = useState(false);
  const [resetSubmitted, setResetSubmitted] = useState(false);
  const [resetError, setResetError] = useState("");
  const resetting = useRef(false);
  const resetStartMessage = useRef<string | null>(null);
  const acting = useRef(false);
  const loadSerial = useRef(0);
  const alive = useRef(true);

  const loadConfig = () => {
    const serial = ++loadSerial.current;
    setLoadError("");
    plcApi.getConfig().then((c) => {
      if (serial !== loadSerial.current) return;
      setSaved(c); setDraft(asS7(structuredClone(c)));
    }).catch(e => { if (serial === loadSerial.current) setLoadError(String(e)); });
  };

  useEffect(() => {
    alive.current = true;
    loadConfig();
    return () => { alive.current = false; loadSerial.current++; };
  }, []);

  useEffect(() => {
    if (!actionError) return;
    const t = setTimeout(() => setActionError(""), 4000);
    return () => clearTimeout(t);
  }, [actionError]);

  const dirty = useMemo(() => configText(saved) !== configText(draft), [saved, draft]);
  const connected = status?.state === "connected";
  const resetRequired = handshake?.phase === "resetRequired" || handshake?.phase === "fault";
  const resetBlockedReason = readOnly ? "当前页面为只读，不能复位"
    : !connected ? "PLC 未连接，连接后才能复位"
    : !resetRequired ? "当前无需复位"
    : handshake.resetBlockedReason === undefined ? "复位条件尚未就绪，等待状态更新"
    : handshake.resetBlockedReason || (busy ? "PLC 操作进行中，请稍后复位" : "");
  const reconnectSaved = !!blockedReason && !!status && !connected;
  const tagLabel = useMemo(() => new Map(tagPresets.map((t) => [t.value, t.label])), [tagPresets]);

  useEffect(() => {
    if (!resetRequired) setResetError("");
    const executionError = resetRequired && handshake?.message !== resetStartMessage.current ? handshake?.message : null;
    if (resetPending && (!resetRequired || !connected || handshake?.resetBlockedReason || executionError)) {
      resetting.current = false;
      setResetPending(false);
      if (executionError) setResetError(executionError);
    }
  }, [resetPending, resetRequired, connected, handshake?.resetBlockedReason, handshake?.message]);

  if (!draft) return <Card><p role={loadError ? "alert" : "status"}>{loadError || "正在加载 PLC 配置…"}</p>{loadError && <Button onClick={loadConfig}>重新加载</Button>}</Card>;

  const canReconnect = () => !!linkState.current && linkState.current !== "connected";
  const resetFault = async () => {
    if (resetting.current || acting.current || resetBlockedReason) return;
    resetting.current = true;
    resetStartMessage.current = handshake?.message ?? null;
    setResetPending(true);
    setResetSubmitted(false);
    setResetError("");
    try {
      await cycleApi.reset();
      if (alive.current) setResetSubmitted(true);
    } catch (error) {
      resetting.current = false;
      if (alive.current) { setResetPending(false); setResetError(String(error)); }
    }
  };
  const perform = async (action: () => Promise<unknown>, allowReconnect = false) => {
    if (acting.current || resetting.current) return;
    if (operationLock.current && !(allowReconnect && canReconnect())) {
      setActionError(operationLock.current); return;
    }
    acting.current = true; setBusy(true); setActionError("");
    try { await action(); }
    catch (e) { if (alive.current) setActionError(String(e)); }
    finally { acting.current = false; if (alive.current) setBusy(false); }
  };

  const invalid = connectionError(draft.connection)
    || (!Number.isSafeInteger(draft.heartbeat.intervalMs) || draft.heartbeat.intervalMs < 0 || (draft.heartbeat.pointId && draft.heartbeat.intervalMs < 100) ? "心跳周期需为整数，启用心跳时不能小于 100 ms" : "")
    || (!Number.isInteger(draft.logRetentionDays) || draft.logRetentionDays < 0 || draft.logRetentionDays > 0xffffffff ? "日志保留天数需为 0–4294967295 的整数" : "");

  const save = async () => {
    if (operationLock.current) { setActionError(operationLock.current); return false; }
    if (invalid) { setActionError(invalid); return false; }
    try {
      await plcApi.saveConfig(draft);
      if (!alive.current) return false;
      setSaved(structuredClone(draft));
      snack("配置已保存");
      return true;
    } catch (e) {
      if (alive.current) setActionError(String(e));
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
    if (operationLock.current) { setActionError(operationLock.current); return; }
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
  const meta = s7Protocol;
  const writablePoints = draft.points.filter((p) => p.access === "readWrite");
  const protectedWrite = (point: PlcPoint) => saved?.connection.protocol === "s7"
    && (point.id === saved.heartbeat.pointId || point.tags.some(tag => inspectionHandshakeTags.has(tag)))
    ? "S7 握手和心跳由检测流程管理，不能手动写入" : "";

  const columns: TableColumn<PlcPoint>[] = [
    { key: "name", header: "名称", render: (p) => <>
      <div className="cell-title">
        {p.name}
        {draft.heartbeat.pointId === p.id && <span className="plc-heartbeat"><Chip>心跳</Chip></span>}
      </div>
      {p.description && <div className="cell-sub">{p.description}</div>}
    </> },
    { key: "address", header: "地址", render: (p) => <Text mono>{p.address}</Text> },
    { key: "dataType", header: "类型", render: (p) => <span className="nowrap">
      {dataTypeLabels[p.dataType].split(" ")[0]}
      {isMultiWord(p.dataType) && <span className="cell-sub"> {effectiveOrder(p)}</span>}
    </span> },
    { key: "access", header: "读写", render: (p) => p.access === "readWrite" ? "读写" : "只读" },
    { key: "edge", header: "边沿", render: (p) => p.edge === "none" ? <span className="muted">—</span> : edgeLabels[p.edge] },
    { key: "tags", header: "标签", render: (p) => p.tags.length === 0 ? <span className="muted">—</span> : <Stack gap={4}>{p.tags.map((t) => <Chip key={t}>{tagLabel.get(t) ?? t}</Chip>)}</Stack> },
    { key: "logChanges", header: "记录", render: (p) => p.logChanges ? "是" : <span className="muted">否</span> },
    { key: "value", header: "当前值", render: (p) => {
      const v = values[p.id];
      const live = savedPoints.get(p.id) === JSON.stringify(p) && !dirty;
      return <span className={`mono plc-value ${live && status?.state === "connected" && v?.error ? "ng" : ""}`}>
        {!live ? <span className="muted">待保存</span> : status?.state === "connected" ? formatValue(v) : <span className="muted">未连接</span>}
      </span>;
    } },
    { key: "actions", header: "操作", align: "right", render: (p) => {
      const live = savedPoints.get(p.id) === JSON.stringify(p) && !dirty;
      return <span className="nowrap">
        <IconButton variant="text" title={protectedWrite(p) || "写入"} aria-label="写入"
          disabled={p.access !== "readWrite" || status?.state !== "connected" || !live || !!protectedWrite(p)} onClick={() => setWriting(p)}>
          <PenLine size={16} />
        </IconButton>
        <IconButton variant="text" title="编辑" aria-label="编辑" onClick={() => setEditing({ point: p, isNew: false })}>
          <Pencil size={16} />
        </IconButton>
        <IconButton variant="text" danger title="删除" aria-label="删除" onClick={() => removePoint(p.id)}>
          <Trash2 size={16} />
        </IconButton>
      </span>;
    } },
  ];

  return (
    <div className="stack plc">
      <section className="plc-actionbar" aria-label="PLC 悬浮操作栏">
        <div className="plc-actionbar-row">
          <div className="plc-actionbar-group">
            <PlcStatusBadge state={status?.state} />
            {dirty && <StatusBadge tone="warn">未保存</StatusBadge>}
            {handshake && <Button leadingIcon={<RotateCcw size={16} />} disabled={!!resetBlockedReason || resetPending}
              loading={resetPending} onClick={() => void resetFault()}>复位故障</Button>}
          </div>
          <fieldset className="plc-actionbar-group" disabled={busy || resetPending || readOnly} style={fieldsetStyle}>
            <Button leadingIcon={<Undo2 size={16} />} disabled={!dirty || !!blockedReason} onClick={() => saved && setDraft(structuredClone(saved))}>还原</Button>
            <Button leadingIcon={<Save size={16} />} disabled={!dirty || !!invalid || !!blockedReason} onClick={() => void perform(save)}>保存配置</Button>
            <Button variant={connected ? "tonal" : "filled"} danger={connected} leadingIcon={connected ? <PlugZap size={16} /> : <Plug size={16} />}
              disabled={!status || (connected ? !!blockedReason : !reconnectSaved && !!invalid)} onClick={toggleConnection}>
              {connected ? "断开" : reconnectSaved ? "使用已保存配置重连" : status?.state === "error" || status?.state === "connecting" ? "重连" : "连接"}
            </Button>
          </fieldset>
          <fieldset className="plc-actionbar-group" disabled={busy || resetPending || readOnly || !!blockedReason} style={fieldsetStyle}>
            <Button onClick={() => setS7Open(true)}>载入一期 S7 点表</Button>
            <Button leadingIcon={<FileJson size={16} />} onClick={() => setJsonOpen(true)}>导入/导出</Button>
            <Button variant="filled" leadingIcon={<Plus size={16} />} onClick={() => setEditing({ point: newPoint(), isNew: true })}>新增点位</Button>
          </fieldset>
          <div className="plc-actionbar-group" ref={setPlanActionsTarget} />
        </div>
        {resetRequired && resetBlockedReason && <Text as="p" variant="label">当前不能复位：{resetBlockedReason}</Text>}
        {resetPending && <Text as="p" role="status">{resetSubmitted ? "复位请求已提交，等待执行结果。" : "正在提交复位请求…"}</Text>}
        {resetError && <Alert tone="ng">复位请求失败：{resetError}</Alert>}
        {actionError && <Alert tone="ng">{actionError}</Alert>}
      </section>
      {blockedReason && <Alert tone="warn">{blockedReason}</Alert>}
      {handshake && <Card role="region" aria-label="S7 握手状态" title={<Heading level={3}>S7 握手状态</Heading>}>
        <Stack gap={16}>
          <span>当前阶段：<b>{handshakePhaseLabels[handshake.phase]} <span className="mono">({handshake.phase})</span></b></span>
          <span>请求序号：<b className="mono">{handshake.requestSeq ?? "—"}</b></span>
          <span>结果序号：<b className="mono">{handshake.resultSeq ?? "—"}</b></span>
        </Stack>
        {handshake.message && <Text as="p" variant="label" muted>{handshake.message}</Text>}
      </Card>}
      <fieldset disabled={busy || resetPending || readOnly} className="stack" style={fieldsetStyle}>
      <Card title={<Stack gap={10}>
          <Heading level={3}>连接</Heading>
          <PlcStatusBadge state={status?.state} />
          <span className="muted ellipsis" title={status?.message}>{status?.message}</span>
        </Stack>}>
        <Stack direction="column" align="stretch" gap={14} wrap={false}>
        {reconnectSaved && <Text as="p" variant="label" muted>重连使用后端已保存的配置，当前草稿不会保存或应用。</Text>}
        <fieldset disabled={!!blockedReason} style={fieldsetStyle}>
        <Stack direction="column" align="stretch" gap={14} wrap={false}>
        <ConnectionForm key={cpuPreset.revision} initialCpuPreset={cpuPreset.key} value={draft.connection} onChange={(connection) => setDraft({ ...draft, connection })} />

        <section className="plc-config-card" aria-labelledby="plc-monitor-title">
        <h4 id="plc-monitor-title">心跳与日志</h4>
        <div className="plc-config-fields">
          <Select label="心跳点位" value={draft.heartbeat.pointId ?? ""}
            onValueChange={(v) => setDraft({ ...draft, heartbeat: { ...draft.heartbeat, pointId: v || null } })}
            options={[{ value: "", label: "不启用" }, ...writablePoints.map((p) => ({ value: p.id, label: `${p.name}（${p.address}）` }))]} />
          <NumberField label="心跳周期 (ms)" min={100} step={1} disabled={!draft.heartbeat.pointId}
            value={Number.isFinite(draft.heartbeat.intervalMs) ? draft.heartbeat.intervalMs : ""}
            onValueChange={(v) => setDraft({ ...draft, heartbeat: { ...draft.heartbeat, intervalMs: v === "" ? NaN : v } })} />
          <NumberField label="日志保留天数（0 为永久）" min={0} step={1}
            value={Number.isFinite(draft.logRetentionDays) ? draft.logRetentionDays : ""}
            onValueChange={(v) => setDraft({ ...draft, logRetentionDays: v === "" ? NaN : v })} />
        </div>
        </section>

        <div className="conn-footer">
          <Checkbox label="启动时自动连接" checked={draft.autoConnect} onCheckedChange={(checked) => setDraft({ ...draft, autoConnect: checked })} />
          <div className="stats-inline">
            <span>周期 <b className="mono">{status?.cycleMs ?? "--"} ms</b></span>
            <span>轮询 <b className="mono">{status?.pollCount ?? 0}</b></span>
            <span>错误 <b className="mono">{status?.errorCount ?? 0}</b></span>
            <span>最近 <b className="mono">{status?.lastPoll ? formatClock(status.lastPoll) : "--"}</b></span>
          </div>
        </div>
        </Stack>
        </fieldset>
        {invalid && !connectionError(draft.connection) && <Alert tone="ng">{invalid}</Alert>}
        </Stack>
      </Card>

      <fieldset disabled={!!blockedReason} style={fieldsetStyle}>
      <Card title={<Stack gap={10}>
          <Heading level={3}>地址表</Heading>
          <span className="muted">{draft.points.length} 个点位</span>
        </Stack>}>
        <Text as="p" variant="label" muted style={{ marginBottom: 10 }}>
          {meta.label} 地址示例：<span className="mono">{meta.examples.join("  ")}</span>
        </Text>
        <Table<PlcPoint> caption="地址表" rows={draft.points} columns={columns} rowKey={(p) => p.id}
          emptyState={<EmptyState title="暂无点位，点击「新增点位」开始配置" />} />
      </Card>
      </fieldset>
      </fieldset>
      <S7RecipePlanPanel actionsTarget={planActionsTarget} />

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
        snack({ text: "一期 S7 点表已载入草稿，请核对地址后保存配置", tone: "info" });
      }} />}
    </div>
  );
}
