import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowRightLeft, Copy, Download, Ellipsis, FolderKanban, Pencil, Plus, RefreshCw, Trash2, Upload } from "lucide-react";
import { Alert, Button, Card, EmptyState, Heading, IconButton, List, ListItem, Menu, StatusBadge, type MenuEntry } from "qing-steel";
import { projectApi, projectTransferApi, type ProjectEntry, type ProjectImportPreview, type ProjectMeta } from "../lib/project";
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

function ProjectActions({ entry, onOpen }: { entry: ProjectEntry; onOpen: (open: Open) => void }) {
  const meta = entry.meta;
  const items: MenuEntry[] = [];
  if (meta) {
    if (!entry.current) items.push({ id: "switch", label: "切换到此项目", icon: <ArrowRightLeft size={16} />, onSelect: () => onOpen({ kind: "switch", project: meta }) });
    items.push(
      { id: "rename", label: "重命名", icon: <Pencil size={16} />, onSelect: () => onOpen({ kind: "rename", project: meta }) },
      { id: "copy", label: "复制项目", icon: <Copy size={16} />, onSelect: () => onOpen({ kind: "copy", project: meta }) },
      { id: "export", label: "导出项目", icon: <Download size={16} />, onSelect: () => onOpen({ kind: "export", project: meta }) },
      { type: "separator", id: "delete-divider" },
    );
  }
  items.push({ id: "delete", label: "删除项目", icon: <Trash2 size={16} />, danger: true, disabled: entry.current, onSelect: () => onOpen({ kind: "delete", entry }) });
  if (entry.current) items.push({ type: "label", id: "delete-hint", label: "请先切换到其他项目" });
  const label = `项目操作：${projectLabel(entry)}`;
  return <Menu label={label} align="end" items={items}
    trigger={<IconButton size="sm" variant="outlined" aria-label={label} title="项目操作"><Ellipsis size={18} /></IconButton>} />;
}

function ProjectDetails({ entry, state, onOpen }: { entry: ProjectEntry; state: UsageState | undefined; onOpen: (open: Open) => void }) {
  const meta = entry.meta;
  const usage = state?.status === "ok" ? state.usage : null;
  return <section className="pj-details" aria-label="项目详情">
    <div className="pj-detail-header">
      <div className="pj-detail-text">
        <Heading level={4} className="pj-detail-title">{projectLabel(entry)}</Heading>
        {meta?.description && <p className="pj-detail-description muted">{meta.description}</p>}
      </div>
      <ProjectActions key={entry.id} entry={entry} onOpen={onOpen} />
    </div>
    {!meta && <Alert tone="warn" title="项目不可用">{entry.error || "project.json 无法读取"}</Alert>}
    {state?.status === "error" && <Alert tone="ng" title="项目统计失败">{state.error}</Alert>}
    {(!state || state.status === "loading") && <p className="pj-usage-status muted" role="status">正在统计项目用量…</p>}
    {usage && <dl className="pj-summary">
      <div><dt>配方</dt><dd>{usage.recipes}</dd></div>
      <div><dt>检测记录</dt><dd>{usage.records} 条</dd></div>
      <div><dt>占用空间</dt><dd>{formatBytes(usage.bytes)}</dd></div>
    </dl>}
    <dl className="kv pj-detail-metadata">
      <dt>编号</dt><dd className="mono">{entry.id}</dd>
      {meta && <><dt>创建时间</dt><dd className="mono">{formatDateTime(meta.createdAt)}</dd></>}
      {usage && usage.records > 0 && usage.firstRecordAt !== null && usage.lastRecordAt !== null && <>
        <dt>记录范围</dt><dd className="mono">{formatDate(usage.firstRecordAt)} 至 {formatDate(usage.lastRecordAt)}</dd>
      </>}
      {usage && usage.projectBytes != null && usage.visionBytes != null && <>
        <dt>空间明细</dt><dd>项目 {formatBytes(usage.projectBytes)} · 视觉 {formatBytes(usage.visionBytes)}</dd>
      </>}
    </dl>
  </section>;
}

export default function ProjectsPage() {
  const available = desktopAvailable();
  const [entries, setEntries] = useState<ProjectEntry[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
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
      if (request === serial.current) {
        setEntries(list);
        setSelectedId(previous => list.some(entry => entry.id === previous) ? previous : list[0]?.id ?? null);
        setError("");
      }
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
  const selected = entries?.find(entry => entry.id === selectedId);
  const usage = useProjectUsage(selected ? [selected.id] : [], version);
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

  return <Card role="region" aria-label="项目" className="pj-page" title={<div className="pj-page-title"><Heading level={3}>项目</Heading><ContextHelp label="项目说明">一个项目是一套完整的检测方案：相机组、PLC、节拍、标定、配方以及它产生的检测记录。同一时刻只运行当前项目。</ContextHelp></div>}
    actions={available && <><IconButton aria-label="刷新项目列表" title="刷新" loading={loading} onClick={refresh}><RefreshCw size={16} /></IconButton><Button variant="outlined" leadingIcon={<Upload size={16} />} loading={picking} disabled={!entries} onClick={() => void pickImport()}>{picking ? "正在读取…" : "导入项目"}</Button><Button variant="filled" leadingIcon={<Plus size={16} />} disabled={!entries} onClick={() => setOpen({ kind: "create" })}>新建项目</Button></>}>
    {!available
      ? <EmptyState icon={<FolderKanban size={30} aria-hidden="true" />} title="项目管理需要桌面后端" description="请在 GlueSight · 胶路智检 桌面软件中查看和管理项目。" />
      : <>
        {pickError && <Alert tone="ng" title="无法读取项目包" onClose={() => setPickError("")}>{pickError}</Alert>}
        {error && <Alert tone="ng" title="项目列表读取失败" action={<Button size="sm" onClick={refresh}>重试</Button>}>{error}</Alert>}
        {selected && entries && <div className="pj-browser">
          <div className="pj-layout">
            <List className="pj-project-list" aria-label="本机项目">
              {entries.map(entry => <ListItem key={entry.id} selected={selected.id === entry.id} onClick={() => setSelectedId(entry.id)}
                title={<span className="pj-project-name">
                  <strong>{projectLabel(entry)}</strong>
                  {entry.current && <StatusBadge tone="ok">当前</StatusBadge>}
                  {!entry.meta && <StatusBadge tone="warn">不可用</StatusBadge>}
                </span>}
                description={entry.meta && <span className="mono">{entry.id}</span>} />)}
            </List>
            <ProjectDetails entry={selected} state={usage[selected.id]} onOpen={setOpen} />
          </div>
        </div>}
        {entries?.length === 0 && <EmptyState title="还没有项目" description="新建一个项目开始配置。" />}
        {!entries && loading && <p role="status" className="muted">正在读取项目列表…</p>}
      </>}
    {open?.kind === "create" && entries && <CreateProjectDialog entries={entries} onClose={close} />}
    {open?.kind === "rename" && entries && <RenameProjectDialog project={open.project} entries={entries} onClose={close} />}
    {open?.kind === "delete" && <DeleteProjectDialog entry={open.entry} usage={usage[open.entry.id]} onClose={close} />}
    {open?.kind === "switch" && <SwitchProjectDialog target={open.project} onClose={close} />}
    {open?.kind === "export" && <ExportProjectDialog project={open.project} onClose={close} />}
    {open?.kind === "copy" && entries && <CopyProjectDialog project={open.project} entries={entries} onClose={close} onSwitch={project => setOpen({ kind: "switch", project })} />}
    {open?.kind === "import" && entries && <ImportProjectDialog preview={open.preview} entries={entries} onClose={close} onSwitch={project => setOpen({ kind: "switch", project })} />}
  </Card>;
}
