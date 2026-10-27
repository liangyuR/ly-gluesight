import { useEffect, useMemo, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { isTauri } from "@tauri-apps/api/core";
import { FileJson, Lock, Pencil, PenLine, Plug, PlugZap, Plus, Save, Trash2, Undo2 } from "lucide-react";
import { Accordion, Button, Card, Chip, Divider, EmptyState, Heading, IconButton, Stack, StatusBadge, Table, Text, type TableColumn } from "qing-steel";
import { connectionError } from "../components/ConnectionForm";
import ConnectionCard from "../components/ConnectionCard";
import HandshakeMonitor, { CommonMonitor } from "../components/HandshakeMonitor";
import PlcNotices, { type PlcNotice } from "../components/PlcNotices";
import PlcStatusBadge from "../components/PlcStatusBadge";
import PointEditor from "../components/PointEditor";
import PointsJsonDialog from "../components/PointsJsonDialog";
import WriteDialog from "../components/WriteDialog";
import S7Phase1Dialog from "../components/S7Phase1Dialog";
import { plcApi, usePlcOperationState, usePlcStatus, usePlcValues } from "../api";
import { commonPlcTags, inspectionHandshakeTags, plcTagHelp } from "../../../business/plcTags";
import ContextHelp from "../../cycle/components/ContextHelp";
import { asS7, cpuPresetLabel, dataTypeLabels, edgeLabels, effectiveOrder, formatValue, isMultiWord, newPoint } from "../meta";
import { formatClock } from "../time";
import { useSnackbar } from "qing-steel";
import type { PlcConfig, PlcPoint, TagPreset } from "../types";

type Editing = { point: PlcPoint; isNew: boolean } | null;

interface PlcPageProps {
  tagPresets?: TagPreset[];
  readOnly?: boolean;
}

const configText = (config: PlcConfig | null) => JSON.stringify(config, (_key, value) =>
  typeof value === "number" && !Number.isFinite(value) ? String(value) : value);
const settingsText = (config: PlcConfig | null) => configText(config && { ...config, points: [] });
const fieldsetStyle = { border: 0, padding: 0, margin: 0, minWidth: 0 };
const isCommonPoint = (point: PlcPoint) => point.tags.some(tag => commonPlcTags.has(tag));
const isHandshakePoint = (point: PlcPoint) => point.tags.some(tag => inspectionHandshakeTags.has(tag));

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
  const [connectionOpen, setConnectionOpen] = useState(false);
  const connectionToggled = useRef(false);
  const [customOpen, setCustomOpen] = useState("");
  const [actionError, setActionError] = useState("");
  const { show: snack } = useSnackbar();
  const [loadError, setLoadError] = useState("");
  const [busy, setBusy] = useState(false);
  const acting = useRef(false);
  const loadSerial = useRef(0);
  const alive = useRef(true);
  const tauri = isTauri();

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
  const settingsDirty = useMemo(() => !!draft && settingsText(saved) !== settingsText(draft), [saved, draft]);
  const savedPoints = useMemo(() => new Map(saved?.points.map((p) => [p.id, JSON.stringify(p)])), [saved]);
  const customPending = !!draft?.points.some(p => !isHandshakePoint(p) && savedPoints.get(p.id) !== JSON.stringify(p));
  // 只看已保存点表：草稿里改了地址的点位还没有在后端轮询。
  const tagPoints = useMemo(() => {
    const map = new Map<string, string>();
    for (const p of saved?.points ?? []) for (const tag of p.tags) if (!map.has(tag)) map.set(tag, p.id);
    return map;
  }, [saved]);
  const tagValue = (tag: string) => {
    const id = tagPoints.get(tag);
    return id === undefined ? undefined : values[id] ?? null;
  };
  const connected = status?.state === "connected";
  const reconnectSaved = !!blockedReason && !!status && !connected;
  const tagLabel = useMemo(() => new Map(tagPresets.map((t) => [t.value, t.label])), [tagPresets]);

  useEffect(() => { if (settingsDirty && !connectionToggled.current) setConnectionOpen(true); }, [settingsDirty]);
  useEffect(() => { if (customPending) setCustomOpen("custom"); }, [customPending]);

  if (!draft) return <Card><p role={loadError ? "alert" : "status"}>{loadError || "正在加载 PLC 配置…"}</p>{loadError && <Button onClick={loadConfig}>重新加载</Button>}</Card>;

  const canReconnect = () => !!linkState.current && linkState.current !== "connected";
  const perform = async (action: () => Promise<unknown>, allowReconnect = false) => {
    if (acting.current) return;
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

  const live = (p: PlcPoint) => savedPoints.get(p.id) === JSON.stringify(p) && !dirty;
  const protectedWrite = (point: PlcPoint) => saved?.connection.protocol === "s7"
    && (point.id === saved.heartbeat.pointId || isHandshakePoint(point))
    ? "S7 握手和心跳由检测流程管理，不能手动写入" : "";
  const lockReason = readOnly ? "离线期间只读" : blockedReason || (busy ? "PLC 操作进行中，请稍候" : "");
  const writeBlocked = (p: PlcPoint) => lockReason || (p.access !== "readWrite" ? "只读点位不能写入"
    : !connected ? "PLC 未连接"
    : !live(p) ? "点位改动保存后才能写入"
    : protectedWrite(p));
  const protocolValue = tagValue("plcVersion");
  const protocolText = connected && protocolValue && !protocolValue.error && protocolValue.value !== null ? String(protocolValue.value) : "—";
  const handshakePoints = draft.points.filter(p => isHandshakePoint(p) && !isCommonPoint(p));
  const commonPoints = draft.points.filter(isCommonPoint);
  const customPoints = draft.points.filter(p => !isHandshakePoint(p));
  const brief = saved ? `S7 · ${cpuPresetLabel(saved.connection)} · ${saved.connection.host}:${saved.connection.port}` : "S7 · 配置未读取";

  const help = (p: PlcPoint) => p.tags.map(tag => plcTagHelp[tag]).find(Boolean);
  const columns = (actions: TableColumn<PlcPoint>["render"]): TableColumn<PlcPoint>[] => [
    { key: "name", header: "名称", render: (p) => <>
      <div className="cell-title">
        {p.name}
        {help(p) && <ContextHelp label={`${p.name}说明`}>{help(p)}</ContextHelp>}
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
      return <span className={`mono plc-value ${live(p) && connected && v?.error ? "ng" : ""}`}>
        {!live(p) ? <span className="muted">待保存</span> : connected ? formatValue(v) : <span className="muted">未连接</span>}
      </span>;
    } },
    { key: "actions", header: "操作", align: "right", render: actions },
  ];
  const editButton = (p: PlcPoint) => <IconButton variant="text" title={lockReason || "编辑"} aria-label="编辑" disabled={!!lockReason} onClick={() => setEditing({ point: p, isNew: false })}>
    <Pencil size={16} />
  </IconButton>;
  const handshakeColumns = columns((p) => editButton(p));
  const customColumns = columns((p) => <span className="nowrap">
    <IconButton variant="text" title={writeBlocked(p) || "写入"} aria-label="写入" disabled={!!writeBlocked(p)} onClick={() => setWriting(p)}>
      <PenLine size={16} />
    </IconButton>
    {editButton(p)}
    <IconButton variant="text" danger title={lockReason || "删除"} aria-label="删除" disabled={!!lockReason} onClick={() => removePoint(p.id)}>
      <Trash2 size={16} />
    </IconButton>
  </span>);
  const loadTemplateButton = <Button disabled={!!lockReason || !tauri} title={lockReason || (!tauri ? "重置 S7 通讯配置需要桌面后端" : undefined)}
    onClick={() => setS7Open(true)}>重置 S7 通讯配置</Button>;
  const pointGroup = (title: string, points: PlcPoint[], detail: string) => <Card role="region" aria-label={title} title={<Heading level={3}>{title}</Heading>}>
    <section className="plc-point-group">
      <div className="plc-group-head">
        <div className="plc-group-title">
          <span className="muted">{points.length} 个 · {detail}</span>
          <Chip><Lock size={12} aria-hidden="true" /> 模板生成，不可手动写入</Chip>
        </div>
        {loadTemplateButton}
      </div>
      <Table<PlcPoint> caption={title} rows={points} columns={handshakeColumns} rowKey={p => p.id}
        emptyState={<EmptyState title={`尚未载入${title}`} description="使用「重置 S7 通讯配置」生成标准点表，核对生产 DB 后保存配置" action={loadTemplateButton} />} />
    </section>
  </Card>;

  const notices: PlcNotice[] = [];
  if (actionError) notices.push({ key: "action", tone: "ng", text: actionError });
  if (loadError) notices.push({ key: "load", tone: "ng", text: `无法读取 PLC 配置：${loadError}`, action: <Button size="sm" onClick={loadConfig}>重新加载</Button> });
  if (status?.state === "error") notices.push({ key: "link", tone: "ng", text: <>{status.message}{status.lastError && <>；最近错误：{status.lastError.message}</>}</> });
  if (blockedReason) notices.push({ key: "blocked", tone: "warn", text: blockedReason });
  if (connected && (handshake?.phase === "fault" || handshake?.phase === "resetRequired")) notices.push({
    key: "reset", tone: "warn", text: `握手待复位${handshake.message ? `：${handshake.message}` : ""}。复位在在线检测页进行`,
    action: <Link className="qs-btn tonal sm" to="/inspect">前往在线检测</Link>,
  });
  if (dirty && invalid) notices.push({ key: "invalid", tone: "warn", text: invalid });
  if (readOnly) notices.push({ key: "readOnly", tone: "info", text: "离线期间 PLC 由站点模式管理，本页只读", secondary: true });
  if (!tauri) notices.push({ key: "preview", tone: "info", text: "浏览器预览模式：PLC 连接、保存、写入需在桌面端运行" });

  const connectDisabledReason = !status ? "正在读取 PLC 状态"
    : !tauri ? "PLC 操作需要桌面后端"
    : connected ? blockedReason
    : reconnectSaved ? "" : invalid;

  return (
    <div className="stack plc">
      <section className="plc-actionbar" aria-label="PLC 状态条">
        <div className="plc-actionbar-row">
          <div className="plc-actionbar-status">
            <PlcStatusBadge state={status?.state} />
            <span className="plc-brief" title={brief}>{brief}</span>
          </div>
          <div className="stats-inline" aria-label="运行指标">
            <span>周期 <b className="mono">{connected && status?.cycleMs != null ? status.cycleMs : "--"} ms</b></span>
            <span>轮询 <b className="mono">{(status?.pollCount ?? 0).toLocaleString("zh-CN")}</b></span>
            <span>错误 <b className="mono">{status?.errorCount ?? 0}</b></span>
            <span>最近 <b className="mono">{connected && status?.lastPoll ? formatClock(status.lastPoll) : "--"}</b></span>
          </div>
          <fieldset className="plc-actionbar-group plc-actionbar-actions" disabled={busy || readOnly} style={{ ...fieldsetStyle, marginLeft: "auto" }}>
            {dirty && <>
              <StatusBadge tone="warn">未保存</StatusBadge>
              <Button leadingIcon={<Undo2 size={16} />} disabled={!!blockedReason} title={blockedReason || undefined}
                onClick={() => saved && setDraft(structuredClone(saved))}>还原</Button>
              <Button leadingIcon={<Save size={16} />} disabled={!!invalid || !!blockedReason || !tauri}
                title={blockedReason || invalid || (!tauri ? "预览模式不能保存" : undefined)} onClick={() => void perform(save)}>保存配置</Button>
              <Divider vertical />
            </>}
            <Button variant={connected ? "tonal" : "filled"} danger={connected} leadingIcon={connected ? <PlugZap size={16} /> : <Plug size={16} />}
              disabled={!!connectDisabledReason} title={connectDisabledReason || undefined} onClick={toggleConnection}>
              {connected ? "断开" : reconnectSaved ? "使用已保存配置重连" : status?.state === "error" || status?.state === "connecting" ? "重连" : "连接"}
            </Button>
          </fieldset>
        </div>
        <PlcNotices notices={notices} />
      </section>

      <CommonMonitor connected={connected} tagValue={tagValue} />

      <fieldset disabled={busy || readOnly} style={fieldsetStyle}>
        <ConnectionCard draft={draft} saved={saved} onChange={setDraft} cpuPreset={cpuPreset}
          open={connectionOpen} onOpenChange={(open) => { connectionToggled.current = true; setConnectionOpen(open); }}
          editBlockedReason={readOnly ? "离线期间只读" : blockedReason} blockedReason={blockedReason}
          connected={connected} reconnectSaved={reconnectSaved} />
      </fieldset>

      <div className="stack plc-flow-panel">
        <HandshakeMonitor handshake={handshake} connected={connected} tagValue={tagValue} />
        {pointGroup("握手点位", handshakePoints, `生产协议 ${protocolText}`)}
      </div>

      <Card role="region" aria-label="地址表" title={<Heading level={3}>地址表</Heading>}>
        <Stack direction="column" align="stretch" gap={16} wrap={false}>
          <section className="plc-point-group">
            <div className="plc-group-head">
              <div className="plc-group-title">
                <span className="plc-group-name">通用点位</span>
                <span className="muted">{commonPoints.length} 个 · 心跳、生产协议版本</span>
                <Chip><Lock size={12} aria-hidden="true" /> 模板生成，不可手动写入</Chip>
              </div>
              <div className="plc-group-actions">
                {loadTemplateButton}
                <Button leadingIcon={<FileJson size={16} />} disabled={!!lockReason} title={lockReason || undefined} onClick={() => setJsonOpen(true)}>导入/导出</Button>
              </div>
            </div>
            <Table<PlcPoint> caption="通用点位" rows={commonPoints} columns={handshakeColumns} rowKey={(p) => p.id}
              emptyState={<EmptyState title="尚未载入通用点位" description="点「重置 S7 通讯配置」生成生产协议 7 点位" action={loadTemplateButton} />} />
          </section>
          <Accordion collapsible className="plc-custom-group" value={customOpen} onValueChange={setCustomOpen} items={[{
            value: "custom",
            title: <span className="plc-group-head">
              <span className="plc-group-title">
                <span className="plc-group-name">自定义点位</span>
                <span className="muted">{customPoints.length} 个</span>
                {customPending && <StatusBadge tone="warn">未保存</StatusBadge>}
              </span>
              <span className="plc-group-actions">
                <Button variant="filled" leadingIcon={<Plus size={16} />} disabled={!!lockReason} title={lockReason || undefined}
                  onClick={(e) => { e.stopPropagation(); setEditing({ point: newPoint(), isNew: true }); }}
                  onKeyDown={(e) => e.stopPropagation()}>新增点位</Button>
              </span>
            </span>,
            content: <Table<PlcPoint> caption="自定义点位" rows={customPoints} columns={customColumns} rowKey={(p) => p.id}
              emptyState={<EmptyState title="没有自定义点位" description="联调需要监视或手动写入的额外地址在这里添加；握手点位不在此列" />} />,
          }]} />
        </Stack>
      </Card>

      {editing && (
        <PointEditor
          initial={editing.point}
          isNew={editing.isNew}
          existingIds={draft.points.map((p) => p.id)}
          connection={draft.connection}
          tagPresets={tagPresets}
          blockedReason={blockedReason}
          templateLocked={!editing.isNew && isHandshakePoint(editing.point)}
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
        snack({ text: "S7 通讯配置已重置到草稿，请核对后保存配置", tone: "info" });
      }} />}
    </div>
  );
}
