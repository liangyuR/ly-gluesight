import { useRef, useState } from "react";
import { Alert, Button, Checkbox, Dialog, RadioGroup, Stack, TextArea, TextField, useSnackbar } from "qing-steel";
import { projectApi, type ProjectEntry, type ProjectMeta } from "../../lib/project";
import { errorText } from "../../lib/errors";
import { notifyProjectsChanged, useAlive, useProjectSwitch, useUnsavedCandidate } from "./hooks";
import { defaultProjectId, formatBytes, projectIdError, projectLabel, projectNameError, recordsText, type UsageState } from "./rules";

const contentOptions = [
  { value: "empty", label: "空项目" },
  { value: "hardware", label: "复制当前项目的硬件配置：相机组、PLC、节拍、飞拍估算参数、标定；不含配方和记录" },
];

export function CreateProjectDialog({ entries, onClose }: { entries: ProjectEntry[]; onClose: () => void }) {
  const snackbar = useSnackbar();
  const alive = useAlive();
  const unsaved = useUnsavedCandidate();
  const switching = useProjectSwitch();
  const [form, setForm] = useState(() => ({ id: defaultProjectId(entries.map(entry => entry.id)), name: "", description: "", copyHardware: true, switchAfter: false }));
  const [nameTouched, setNameTouched] = useState(false);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState("");
  const [created, setCreated] = useState<ProjectMeta | null>(null);
  const pending = useRef(false);
  const idError = created ? "" : projectIdError(form.id, entries);
  const nameError = created ? "" : projectNameError(form.name, entries);
  const blockedBySwitch = form.switchAfter ? unsaved : "";
  const busy = working || switching.restarting;
  const locked = busy || !!created;
  const submit = async () => {
    if (pending.current || busy || idError || nameError || blockedBySwitch) return;
    pending.current = true;
    setWorking(true);
    setError("");
    try {
      const meta = created ?? await projectApi.create({ id: form.id, name: form.name.trim(), description: form.description.trim(), copyHardware: form.copyHardware });
      if (!created) notifyProjectsChanged();
      if (!alive.current) return;
      setCreated(meta);
      if (!form.switchAfter) { snackbar.show({ text: `已创建项目“${meta.name}”`, tone: "ok" }); onClose(); return; }
      setWorking(false);
      await switching.run(meta.id);
    } catch (e) {
      if (alive.current) setError(errorText(e));
    } finally {
      pending.current = false;
      if (alive.current) setWorking(false);
    }
  };
  const label = switching.restarting ? "正在重启…" : working ? "创建中…" : created ? "重试切换" : form.switchAfter ? "创建并切换" : "创建";
  return <Dialog open title="新建项目" size="md" closeDisabled={busy} onOpenChange={open => { if (!open && !busy) onClose(); }}
    footer={<><Button disabled={busy} onClick={onClose}>{created ? "关闭" : "取消"}</Button><Button variant="filled" disabled={busy || !!idError || !!nameError || !!blockedBySwitch} onClick={() => void submit()}>{label}</Button></>}>
    <Stack direction="column" align="stretch" gap={14} wrap={false}>
      <TextField label="名称" required disabled={locked} value={form.name} error={nameTouched ? nameError : undefined} onBlur={() => setNameTouched(true)} onChange={e => { setNameTouched(true); setForm({ ...form, name: e.target.value }); }} />
      <TextField label="编号" className="mono" disabled={locked} maxLength={64} value={form.id} description="创建后不可修改，同时作为项目目录名" error={idError || undefined} onChange={e => setForm({ ...form, id: e.target.value })} />
      <TextArea label="描述" disabled={locked} rows={2} value={form.description} onChange={e => setForm({ ...form, description: e.target.value })} />
      <RadioGroup label="初始内容" direction="column" disabled={locked} value={form.copyHardware ? "hardware" : "empty"} options={contentOptions} onValueChange={value => setForm({ ...form, copyHardware: value === "hardware" })} />
      <Checkbox label="创建后切换到该项目（会重启软件）" disabled={locked} checked={form.switchAfter} onCheckedChange={checked => setForm({ ...form, switchAfter: checked })} />
      {blockedBySwitch && !created && <Alert tone="warn" title="创建后无法立即切换">{blockedBySwitch}</Alert>}
      {error && <Alert tone="ng" title="创建未完成">{error}</Alert>}
      {created && switching.error && <Alert tone="ng" title="项目已创建，切换未完成">{switching.error}</Alert>}
      {created && blockedBySwitch && <Alert tone="ng" title="项目已创建，暂不能切换">{blockedBySwitch}</Alert>}
      {switching.restarting && <Alert tone="info" role="status">正在重启…</Alert>}
    </Stack>
  </Dialog>;
}

export function RenameProjectDialog({ project, entries, onClose }: { project: ProjectMeta; entries: ProjectEntry[]; onClose: () => void }) {
  const snackbar = useSnackbar();
  const alive = useAlive();
  const [name, setName] = useState(project.name);
  const [description, setDescription] = useState(project.description);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState("");
  const pending = useRef(false);
  const nameError = projectNameError(name, entries, project.id);
  const submit = async () => {
    if (pending.current || nameError) return;
    pending.current = true;
    setWorking(true);
    setError("");
    try {
      const meta = await projectApi.rename(project.id, name.trim(), description.trim());
      notifyProjectsChanged();
      if (!alive.current) return;
      snackbar.show({ text: `项目已更名为“${meta.name}”`, tone: "ok" });
      onClose();
    } catch (e) {
      if (alive.current) setError(errorText(e));
    } finally {
      pending.current = false;
      if (alive.current) setWorking(false);
    }
  };
  return <Dialog open title="重命名项目" description={`编号 ${project.id} 不变`} closeDisabled={working} onOpenChange={open => { if (!open && !working) onClose(); }}
    footer={<><Button disabled={working} onClick={onClose}>取消</Button><Button variant="filled" disabled={working || !!nameError} onClick={() => void submit()}>{working ? "保存中…" : "保存"}</Button></>}>
    <Stack direction="column" align="stretch" gap={14} wrap={false}>
      <TextField label="名称" required disabled={working} value={name} error={nameError || undefined} onChange={e => setName(e.target.value)} />
      <TextArea label="描述" disabled={working} rows={2} value={description} onChange={e => setDescription(e.target.value)} />
      {error && <Alert tone="ng" title="保存未完成">{error}</Alert>}
    </Stack>
  </Dialog>;
}

export function DeleteProjectDialog({ entry, usage, onClose }: { entry: ProjectEntry; usage: UsageState | undefined; onClose: () => void }) {
  const snackbar = useSnackbar();
  const alive = useAlive();
  const [typed, setTyped] = useState("");
  const [working, setWorking] = useState(false);
  const [error, setError] = useState("");
  const pending = useRef(false);
  const expected = projectLabel(entry);
  const confirmed = typed === expected;
  const submit = async () => {
    if (pending.current || !confirmed) return;
    pending.current = true;
    setWorking(true);
    setError("");
    try {
      await projectApi.remove(entry.id);
      notifyProjectsChanged();
      if (!alive.current) return;
      snackbar.show({ text: `已删除项目“${expected}”`, tone: "ok" });
      onClose();
    } catch (e) {
      if (alive.current) setError(errorText(e));
    } finally {
      pending.current = false;
      if (alive.current) setWorking(false);
    }
  };
  return <Dialog open title="删除项目" closeDisabled={working} onOpenChange={open => { if (!open && !working) onClose(); }}
    footer={<><Button disabled={working} onClick={onClose}>取消</Button><Button danger variant="filled" disabled={working || !confirmed} onClick={() => void submit()}>{working ? "删除中…" : "删除"}</Button></>}>
    <Stack direction="column" align="stretch" gap={14} wrap={false}>
      <Alert tone="ng" title="将永久删除以下内容，不可恢复">
        <p>包括本项目在日志目录中的视觉原图。</p>
        <dl className="kv pj-delete-list">
          <dt>项目</dt><dd>{entry.meta ? <>{expected} <span className="mono">{entry.id}</span></> : <span className="mono">{entry.id}</span>}</dd>
          {usage?.status === "ok" ? <>
            <dt>配方</dt><dd>{usage.usage.recipes} 个</dd>
            <dt>候选</dt><dd>{usage.usage.workspaces} 个</dd>
            <dt>采集轮次</dt><dd>{usage.usage.captureRounds} 轮</dd>
            <dt>检测记录</dt><dd>{recordsText(usage.usage)}</dd>
            <dt>占用空间</dt><dd>{formatBytes(usage.usage.bytes)}</dd>
          </> : <><dt>内容</dt><dd>{usage?.status === "error" ? `统计失败：${usage.error}` : "统计计算中"}</dd></>}
        </dl>
      </Alert>
      <TextField label={entry.meta ? `输入项目名称“${expected}”以确认` : `输入项目编号“${expected}”以确认`} disabled={working} value={typed} autoComplete="off" onChange={e => setTyped(e.target.value)} />
      {error && <Alert tone="ng" title="删除未完成">{error}</Alert>}
    </Stack>
  </Dialog>;
}
