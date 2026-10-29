import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { FolderKanban, Plus, RefreshCw, Upload } from "lucide-react";
import { Alert, Button, Card, EmptyState, Heading, IconButton, StatusBadge, Table, Text, Tooltip, type TableColumn } from "qing-steel";
import { projectApi, projectTransferApi, type ProjectEntry, type ProjectImportPreview, type ProjectMeta, type ProjectUsage } from "../lib/project";
import { desktopAvailable } from "../lib/desktop";
import { errorText } from "../lib/errors";
import { onProjectsChanged, useProjectUsage } from "../features/project/hooks";
import { formatBytes, formatDate, formatDateTime, projectLabel, type UsageState } from "../features/project/rules";
import { CreateProjectDialog, DeleteProjectDialog, RenameProjectDialog } from "../features/project/ProjectDialogs";
import SwitchProjectDialog from "../features/project/SwitchProjectDialog";
import { CopyProjectDialog, ExportProjectDialog, ImportProjectDialog } from "../features/project/TransferDialogs";
import ContextHelp from "../features/cycle/components/ContextHelp";
import "../features/project/project.css";

type Open = { kind: "create" } | { kind: "rename"; project: ProjectMeta } | { kind: "delete"; entry: ProjectEntry } | { kind: "switch"; project: ProjectMeta }
  | { kind: "export"; project: ProjectMeta } | { kind: "copy"; project: ProjectMeta } | { kind: "import"; preview: ProjectImportPreview };

function UsageCell({ state, format }: { state: UsageState | undefined; format: (usage: ProjectUsage) => ReactNode }) {
  if (!state || state.status === "loading") return <span className="muted">计算中</span>;
  if (state.status === "error") return <Tooltip pinnable content={`统计失败：${state.error}`}><Button variant="text" tone="danger" size="sm" className="pj-usage-error">统计失败：{state.error}</Button></Tooltip>;
  return <span className="mono">{format(state.usage)}</span>;
}

function records(usage: ProjectUsage) {
  return <span className="pj-records">{usage.records} 条{usage.records > 0 && usage.firstRecordAt !== null && usage.lastRecordAt !== null && <small className="muted">{formatDate(usage.firstRecordAt)} – {formatDate(usage.lastRecordAt)}</small>}</span>;
}

export default function ProjectsPage() {
  const available = desktopAvailable();
  const [entries, setEntries] = useState<ProjectEntry[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [version, setVersion] = useState(0);
  const [open, setOpen] = useState<Open | null>(null);
  const [picking, setPicking] = useState(false);
  const [pickError, setPickError] = useState("");
  const serial = useRef(0);
  const reload = useCallback(async () => {
    if (!desktopAvailable()) return;
    const request = ++serial.current;
    setLoading(true);
    try {
      const list = await projectApi.list();
      if (request === serial.current) { setEntries(list); setError(""); }
    } catch (e) {
      if (request === serial.current) setError(errorText(e));
    } finally {
      if (request === serial.current) setLoading(false);
    }
  }, []);
  useEffect(() => {
    void reload();
    const stop = onProjectsChanged(() => void reload());
    return () => { serial.current++; stop(); };
  }, [reload]);
  const usage = useProjectUsage(entries?.map(entry => entry.id) ?? [], version);
  const refresh = () => { setVersion(v => v + 1); void reload(); };
  const close = () => setOpen(null);
  const pickImport = async () => {
    if (picking) return;
    setPicking(true);
    setPickError("");
    try {
      const preview = await projectTransferApi.pickImport();
      if (preview) setOpen({ kind: "import", preview });
    } catch (e) {
      setPickError(errorText(e));
    } finally {
      setPicking(false);
    }
  };

  const columns: TableColumn<ProjectEntry>[] = [
    { key: "name", header: "名称", rowHeader: true, render: entry => <div className="pj-name">
      <span className="pj-name-line"><strong>{projectLabel(entry)}</strong>{entry.current && <StatusBadge tone="ok">当前</StatusBadge>}{!entry.meta && <StatusBadge tone="warn">不可用</StatusBadge>}</span>
      {entry.meta?.description && <Text as="small" muted>{entry.meta.description}</Text>}
      {!entry.meta && <Text as="small" className="ng">{entry.error || "project.json 无法读取"}</Text>}
    </div> },
    { key: "id", header: "编号", nowrap: true, render: entry => <span className="mono">{entry.id}</span> },
    { key: "createdAt", header: "创建时间", nowrap: true, render: entry => entry.meta ? <span className="mono">{formatDateTime(entry.meta.createdAt)}</span> : <span className="muted">--</span> },
    { key: "recipes", header: "配方", align: "right", nowrap: true, colSpan: entry => usage[entry.id]?.status === "error" ? 3 : 1, render: entry => <UsageCell state={usage[entry.id]} format={u => String(u.recipes)} /> },
    { key: "records", header: "检测记录", render: entry => <UsageCell state={usage[entry.id]} format={records} /> },
    { key: "bytes", header: "占用空间", align: "right", nowrap: true, render: entry => <UsageCell state={usage[entry.id]} format={u => <span className="pj-storage-usage">
      <span>{formatBytes(u.bytes)}</span>
      {u.projectBytes != null && u.visionBytes != null && <small className="muted">项目 {formatBytes(u.projectBytes)} · 视觉 {formatBytes(u.visionBytes)}</small>}
    </span>} /> },
    { key: "actions", header: "操作", nowrap: true, render: entry => {
      const meta = entry.meta;
      return <div className="pj-actions">
        {meta && !entry.current && <Button size="sm" variant="tonal" onClick={() => setOpen({ kind: "switch", project: meta })}>切换</Button>}
        {meta && <Button size="sm" variant="text" onClick={() => setOpen({ kind: "rename", project: meta })}>重命名</Button>}
        {meta && <Button size="sm" variant="text" onClick={() => setOpen({ kind: "export", project: meta })}>导出</Button>}
        {meta && entries && <Button size="sm" variant="text" onClick={() => setOpen({ kind: "copy", project: meta })}>复制</Button>}
        {entry.current
          ? <Tooltip content="先切换到其他项目后才能删除"><span tabIndex={0} className="pj-disabled-action" aria-label="删除不可用：先切换到其他项目"><Button size="sm" variant="text" danger disabled aria-label="删除（先切换到其他项目）">删除</Button></span></Tooltip>
          : <Button size="sm" variant="text" danger onClick={() => setOpen({ kind: "delete", entry })}>删除</Button>}
      </div>;
    } },
  ];

  return <Card role="region" aria-label="项目" className="pj-page" title={<div style={{ display: "flex", alignItems: "center" }}><Heading level={3}>项目</Heading><ContextHelp label="项目说明">一个项目是一套完整的检测方案：相机组、PLC、节拍、标定、配方以及它产生的检测记录。同一时刻只运行当前项目。</ContextHelp></div>}
    actions={available && <><IconButton aria-label="刷新项目列表" title="刷新" loading={loading} onClick={refresh}><RefreshCw size={16} /></IconButton><Button variant="outlined" leadingIcon={<Upload size={16} />} loading={picking} disabled={!entries} onClick={() => void pickImport()}>{picking ? "正在读取…" : "导入项目"}</Button><Button variant="filled" leadingIcon={<Plus size={16} />} disabled={!entries} onClick={() => setOpen({ kind: "create" })}>新建项目</Button></>}>
    {!available
      ? <EmptyState icon={<FolderKanban size={30} aria-hidden="true" />} title="项目管理需要桌面后端" description="请在 GlueSight · 胶路智检 桌面软件中查看和管理项目。" />
      : <>
        {pickError && <Alert tone="ng" title="无法读取项目包" onClose={() => setPickError("")}>{pickError}</Alert>}
        {error && <Alert tone="ng" title="项目列表读取失败" action={<Button size="sm" onClick={refresh}>重试</Button>}>{error}</Alert>}
        {entries && <Table<ProjectEntry> caption="本机项目" rows={entries} columns={columns} rowKey={entry => entry.id} loading={loading} keepRowsWhileLoading
          getRowProps={entry => entry.meta ? undefined : { tone: "warn" }}
          emptyState={<EmptyState title="还没有项目" description="新建一个项目开始配置。" />} />}
        {!entries && loading && <p role="status" className="muted">正在读取项目列表…</p>}
      </>}
    {open?.kind === "create" && entries && <CreateProjectDialog entries={entries} onClose={close} />}
    {open?.kind === "rename" && entries && <RenameProjectDialog project={open.project} entries={entries} onClose={close} />}
    {open?.kind === "delete" && <DeleteProjectDialog entry={open.entry} usage={usage[open.entry.id]} onClose={close} />}
    {open?.kind === "switch" && <SwitchProjectDialog target={open.project} onClose={close} />}
    {open?.kind === "export" && <ExportProjectDialog project={open.project} usage={usage[open.project.id]} onClose={close} />}
    {open?.kind === "copy" && entries && <CopyProjectDialog project={open.project} usage={usage[open.project.id]} entries={entries} onClose={close} onSwitch={project => setOpen({ kind: "switch", project })} />}
    {open?.kind === "import" && entries && <ImportProjectDialog preview={open.preview} entries={entries} onClose={close} onSwitch={project => setOpen({ kind: "switch", project })} />}
  </Card>;
}
