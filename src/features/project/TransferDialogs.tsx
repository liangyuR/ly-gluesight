import { useEffect, useRef, useState } from "react";
import { Alert, Button, Checkbox, Dialog, Stack, Text, TextArea, TextField } from "qing-steel";
import { projectTransferApi, type ProjectEntry, type ProjectExported, type ProjectImported, type ProjectImportPreview, type ProjectMeta, type TransferSize } from "../../lib/project";
import { errorText } from "../../lib/errors";
import { notifyProjectsChanged, useAlive } from "./hooks";
import { defaultProjectId, formatBytes, formatDateTime, importedProjectName, projectIdError, projectNameError } from "./rules";

function Notes({ notes }: { notes: string[] }) {
  if (!notes.length) return null;
  return <ul className="pj-notes">{notes.map(note => <li key={note}>{note}</li>)}</ul>;
}

type SizeState = { status: "loading" } | { status: "ok"; size: TransferSize } | { status: "error"; error: string };

function useTransferSize(id: string): SizeState {
  const [state, setState] = useState<SizeState>({ status: "loading" });
  useEffect(() => {
    let live = true;
    setState({ status: "loading" });
    projectTransferApi.transferSize(id).then(
      size => { if (live) setState({ status: "ok", size }); },
      e => { if (live) setState({ status: "error", error: errorText(e) }); },
    );
    return () => { live = false; };
  }, [id]);
  return state;
}

function sizeText(state: SizeState, captureImages: boolean, includeRecords = false): string {
  if (state.status === "loading") return "计算中";
  if (state.status === "error") return `统计失败：${state.error}`;
  const { content, captureImages: images, records } = state.size;
  return `约 ${formatBytes(content + (captureImages ? images : 0) + (includeRecords ? records : 0))}（压缩前）`;
}

const CAPTURE_IMAGES_HINT = "不带示教件原图时，已发布的配方照常检测；要看候选的示教图、示教走廊、测量示教件或试测，需在示教件页从历史记录补回原图（要同时带上检测记录与历史原图），否则只能重新选用示教件。";
const COPY_CAPTURE_IMAGES_HINT = "不带示教件原图时，已发布的配方照常检测；要看候选的示教图、示教走廊、测量示教件或试测，需在示教件页从历史记录补回原图，否则只能重新选用示教件。";

export function ExportProjectDialog({ project, onClose }: { project: ProjectMeta; onClose: () => void }) {
  const alive = useAlive();
  const size = useTransferSize(project.id);
  const [includeRecords, setIncludeRecords] = useState(false);
  const [captureImages, setCaptureImages] = useState(false);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState("");
  const [exported, setExported] = useState<ProjectExported | null>(null);
  const pending = useRef(false);
  const submit = async () => {
    if (pending.current) return;
    pending.current = true;
    setWorking(true);
    setError("");
    try {
      const result = await projectTransferApi.exportProject(project.id, includeRecords, captureImages);
      if (alive.current && result) setExported(result);
    } catch (e) {
      if (alive.current) setError(errorText(e));
    } finally {
      pending.current = false;
      if (alive.current) setWorking(false);
    }
  };
  if (exported) return <Dialog open title="已导出项目" onOpenChange={open => { if (!open) onClose(); }}
    footer={<Button variant="filled" onClick={onClose}>完成</Button>}>
    <Stack direction="column" align="stretch" gap={12} wrap={false}>
      <dl className="kv pj-transfer-list">
        <dt>项目</dt><dd>{project.name} <span className="mono">{project.id}</span></dd>
        <dt>保存位置</dt><dd className="mono pj-path">{exported.path}</dd>
        <dt>大小</dt><dd>{formatBytes(exported.bytes)} · {exported.files} 个文件</dd>
        <dt>检测记录</dt><dd>{exported.includesRecords ? "已包含本项目的检测记录与历史原图" : "未包含"}</dd>
        <dt>示教件原图</dt><dd>{captureImages ? "已包含" : "未包含"}</dd>
      </dl>
      <Notes notes={exported.notes} />
    </Stack>
  </Dialog>;
  return <Dialog open title="导出项目" closeDisabled={working} onOpenChange={open => { if (!open && !working) onClose(); }}
    footer={<><Button disabled={working} onClick={onClose}>取消</Button><Button variant="filled" loading={working} disabled={working} onClick={() => void submit()}>{working ? "正在导出…" : "选择位置并导出"}</Button></>}>
    <Stack direction="column" align="stretch" gap={12} wrap={false}>
      <dl className="kv pj-transfer-list">
        <dt>项目</dt><dd>{project.name}</dd>
        <dt>编号</dt><dd className="mono">{project.id}</dd>
        <dt>导出大小</dt><dd>{sizeText(size, captureImages, includeRecords)}</dd>
      </dl>
      <Checkbox label="包含示教件原图" disabled={working} checked={captureImages} onCheckedChange={setCaptureImages} />
      <Checkbox label="包含本项目的检测记录与历史原图" disabled={working} checked={includeRecords} onCheckedChange={setIncludeRecords} />
      <Text muted>导出项目的相机组与 PLC 配置、标定、生产配方与发布包、候选；默认不含示教件原图、检测记录与历史原图。{!captureImages && CAPTURE_IMAGES_HINT}检测记录本机共用，包含时只带本项目的记录与历史原图。只把示教好的配方送回原项目时，在配方库导出工件配方，不用导出整个项目。</Text>
      {includeRecords && <Alert tone="warn" title="导出包可能很大">检测记录与历史原图可能有几十 GB，导出需要较长时间，请确认目标磁盘空间充足。</Alert>}
      {error && <Alert tone="ng" title="导出未完成">{error}</Alert>}
      {working && <Alert tone="info" role="status">正在导出，请勿关闭软件。</Alert>}
    </Stack>
  </Dialog>;
}

export function ImportProjectDialog({ preview, entries, onClose, onSwitch }: { preview: ProjectImportPreview; entries: ProjectEntry[]; onClose: () => void; onSwitch: (project: ProjectMeta) => void }) {
  const alive = useAlive();
  const [form, setForm] = useState(() => ({ id: defaultProjectId(entries.map(entry => entry.id)), name: importedProjectName(preview.name, entries) }));
  const [working, setWorking] = useState(false);
  const [error, setError] = useState("");
  const [imported, setImported] = useState<ProjectImported | null>(null);
  const pending = useRef(false);
  const idError = projectIdError(form.id, entries);
  const nameError = projectNameError(form.name, entries);
  const submit = async () => {
    if (pending.current || idError || nameError) return;
    pending.current = true;
    setWorking(true);
    setError("");
    try {
      const result = await projectTransferApi.importProject(preview.path, form.id, form.name.trim());
      notifyProjectsChanged();
      if (alive.current) setImported(result);
    } catch (e) {
      if (alive.current) setError(errorText(e));
    } finally {
      pending.current = false;
      if (alive.current) setWorking(false);
    }
  };
  if (imported) return <Dialog open title="已导入项目" onOpenChange={open => { if (!open) onClose(); }}
    footer={<><Button onClick={onClose}>完成</Button><Button variant="tonal" onClick={() => onSwitch(imported.meta)}>切换到该项目</Button></>}>
    <Stack direction="column" align="stretch" gap={12} wrap={false}>
      <dl className="kv pj-transfer-list">
        <dt>项目</dt><dd>{imported.meta.name}</dd>
        <dt>编号</dt><dd className="mono">{imported.meta.id}</dd>
      </dl>
      <Alert tone="warn" title="切换前请先核对">相机组、PLC 配置与标定来自来源机，切换到该项目前请先核对；生产配方的计划号已按本机重新分配，候选已回到草稿。</Alert>
      <Notes notes={imported.notes} />
    </Stack>
  </Dialog>;
  return <Dialog open title="导入项目" initialFocus="first-field" size="md" closeDisabled={working} onOpenChange={open => { if (!open && !working) onClose(); }}
    footer={<><Button disabled={working} onClick={onClose}>取消</Button><Button variant="filled" loading={working} disabled={working || !!idError || !!nameError} onClick={() => void submit()}>{working ? "正在导入…" : "导入为新项目"}</Button></>}>
    <Stack direction="column" align="stretch" gap={14} wrap={false}>
      <dl className="kv pj-transfer-list">
        <dt>来源项目</dt><dd>{preview.name}</dd>
        <dt>来源编号</dt><dd className="mono">{preview.sourceId}</dd>
        {preview.description && <><dt>描述</dt><dd>{preview.description}</dd></>}
        <dt>导出时间</dt><dd className="mono">{formatDateTime(preview.exportedAt)}</dd>
        <dt>软件版本</dt><dd className="mono">{preview.appVersion || "未知"}</dd>
        <dt>检测记录</dt><dd>{preview.includesRecords ? "包含检测记录与历史原图" : "不包含"}</dd>
        <dt>大小</dt><dd>{formatBytes(preview.bytes)} · {preview.files} 个文件</dd>
      </dl>
      <Notes notes={preview.notes} />
      <TextField label="新项目编号" className="mono" disabled={working} maxLength={64} value={form.id} description="导入后不可修改，同时作为项目目录名" error={idError || undefined} onChange={e => setForm({ ...form, id: e.target.value })} />
      <TextField label="新项目名称" required disabled={working} value={form.name} error={nameError || undefined} onChange={e => setForm({ ...form, name: e.target.value })} />
      <Alert tone="warn" title="导入后不会自动切换">导入后请先核对相机组、PLC 配置与标定再切换到该项目；生产配方的计划号会按本机重新分配，候选回到草稿。</Alert>
      {error && <Alert tone="ng" title="导入未完成">{error}</Alert>}
      {working && <Alert tone="info" role="status">正在导入，包较大时需要几分钟，请勿关闭软件。</Alert>}
    </Stack>
  </Dialog>;
}

export function CopyProjectDialog({ project, entries, onClose, onSwitch }: { project: ProjectMeta; entries: ProjectEntry[]; onClose: () => void; onSwitch: (project: ProjectMeta) => void }) {
  const alive = useAlive();
  const size = useTransferSize(project.id);
  const [form, setForm] = useState(() => ({ id: defaultProjectId(entries.map(entry => entry.id)), name: importedProjectName(project.name, entries, "副本"), description: project.description, captureImages: false }));
  const [working, setWorking] = useState(false);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState<ProjectImported | null>(null);
  const pending = useRef(false);
  const idError = projectIdError(form.id, entries);
  const nameError = projectNameError(form.name, entries);
  const submit = async () => {
    if (pending.current || idError || nameError) return;
    pending.current = true;
    setWorking(true);
    setError("");
    try {
      const result = await projectTransferApi.copyProject({ sourceId: project.id, id: form.id, name: form.name.trim(), description: form.description.trim(), captureImages: form.captureImages });
      notifyProjectsChanged();
      if (alive.current) setCopied(result);
    } catch (e) {
      if (alive.current) setError(errorText(e));
    } finally {
      pending.current = false;
      if (alive.current) setWorking(false);
    }
  };
  if (copied) return <Dialog open title="已复制项目" onOpenChange={open => { if (!open) onClose(); }}
    footer={<><Button onClick={onClose}>完成</Button><Button variant="tonal" onClick={() => onSwitch(copied.meta)}>切换到该项目</Button></>}>
    <Stack direction="column" align="stretch" gap={12} wrap={false}>
      <dl className="kv pj-transfer-list">
        <dt>项目</dt><dd>{copied.meta.name}</dd>
        <dt>编号</dt><dd className="mono">{copied.meta.id}</dd>
        <dt>复制自</dt><dd>{project.name} <span className="mono">{project.id}</span></dd>
      </dl>
      <Notes notes={copied.notes} />
    </Stack>
  </Dialog>;
  return <Dialog open title="复制项目" initialFocus="first-field" size="md" closeDisabled={working} onOpenChange={open => { if (!open && !working) onClose(); }}
    footer={<><Button disabled={working} onClick={onClose}>取消</Button><Button variant="filled" loading={working} disabled={working || !!idError || !!nameError} onClick={() => void submit()}>{working ? "正在复制…" : "复制为新项目"}</Button></>}>
    <Stack direction="column" align="stretch" gap={14} wrap={false}>
      <dl className="kv pj-transfer-list">
        <dt>来源项目</dt><dd>{project.name} <span className="mono">{project.id}</span></dd>
        <dt>复制大小</dt><dd>{sizeText(size, form.captureImages)}</dd>
      </dl>
      <TextField label="新项目名称" required disabled={working} value={form.name} error={nameError || undefined} onChange={e => setForm({ ...form, name: e.target.value })} />
      <TextField label="新项目编号" className="mono" disabled={working} maxLength={64} value={form.id} description="创建后不可修改，同时作为项目目录名" error={idError || undefined} onChange={e => setForm({ ...form, id: e.target.value })} />
      <TextArea label="描述" disabled={working} rows={2} value={form.description} onChange={e => setForm({ ...form, description: e.target.value })} />
      <Checkbox label="包含示教件原图" disabled={working} checked={form.captureImages} onCheckedChange={captureImages => setForm({ ...form, captureImages })} />
      <Text muted>复制相机组与 PLC 配置、标定、生产配方与发布包、候选。生产配方的计划号按本机重新分配，候选回到草稿；复制后不会自动切换。检测记录与历史原图本机共用、不复制：仍归来源项目，新项目的候选照常引用。{!form.captureImages && COPY_CAPTURE_IMAGES_HINT}</Text>
      {error && <Alert tone="ng" title="复制未完成">{error}</Alert>}
      {working && <Alert tone="info" role="status">正在复制，项目较大时需要几分钟，请勿关闭软件。</Alert>}
    </Stack>
  </Dialog>;
}
