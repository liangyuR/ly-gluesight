import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Button, Checkbox, Chip, Pagination, SegmentedControl, Select, Table, TextField, type TableColumn } from "qing-steel";
import { errorText } from "../../lib/errors";
import { useRecipes } from "../cycle";
import { partVerdict, verdictGroups } from "../history";
import type { PartMode } from "../history/types";
import { useProjectDirectory } from "../project/hooks";
import { projectLabel } from "../project/rules";
import { workspaceApi } from "./api";
import { burstImageApi } from "./burstImages";
import { Badge, Notice, Panel } from "./components";
import { FrameBrowser } from "./FrameBrowser";
import { pieceTime } from "./pieces";
import type { HistoryCandidate, HistoryCandidatePage, HistoryCandidateQuery } from "./types";

const modeLabels: Record<PartMode, string> = { production: "生产", maintenanceCollect: "维护采集", maintenanceShadow: "维护试判" };
const usageLabels = { teaching: "已使用（示教件）", calibration: "已使用（标定件）" } as const;
const PAGE = 50;
const ranges: { value: string; label: string; days: number | null }[] = [
  { value: "all", label: "全部", days: null },
  { value: "today", label: "今天", days: 0 },
  { value: "7d", label: "7 天", days: 7 },
  { value: "30d", label: "30 天", days: 30 },
];
/** 项目筛选选“当前项目”的取值；项目编号只含字母、数字、- 和 _，不会与它重名 */
const CURRENT_PROJECT = "@current";

function rangeStart(days: number | null) {
  if (days === null) return null;
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime() - days * 86_400_000;
}

function timing(r: HistoryCandidate) {
  if (r.durationMs == null && r.measuredFps == null) return "—";
  const text = [r.durationMs != null && `${(r.durationMs / 1000).toFixed(1)} s`, r.measuredFps != null && `${r.measuredFps.toFixed(1)} fps`].filter(Boolean).join(" · ") + (r.setFps != null ? `（设定 ${r.setFps}）` : "");
  const off = r.measuredFps != null && r.setFps != null && r.setFps > 0 && Math.abs(r.measuredFps - r.setFps) > 0.1 * r.setFps;
  return off ? <Badge tone="warn">{text}</Badge> : text;
}

/** 不能选的原因：不可用原因在前，已被本候选使用的标“已使用” */
const blockers = (r: HistoryCandidate) => [...r.reasons, ...(r.usage ? [usageLabels[r.usage]] : [])];

interface Props {
  recipeId: string;
  title: string;
  help?: ReactNode;
  disabled: boolean;
  confirmLabel: string;
  nameOf: (cameraId: string) => string;
  /** 提交所选的一件；返回 true 表示已完成，清空勾选并重读历史件 */
  onConfirm: (recordId: number) => Promise<boolean>;
  onError: (message: string) => void;
}

/**
 * 历史件选择器（RB-6，示教件页选一件）：本机全部真机录制件（不限项目、配方），按项目、配方、SN、时间、判定筛选并分页；
 * 逐件给出能否选用的原因，不可用或已被本候选使用的不能选。勾选或点“看图”的那一件在旁边按相机、帧组预览录制原图（只读，不写候选）。
 */
export default function HistoryPicker({ recipeId, title, help, disabled, confirmLabel, nameOf, onConfirm, onError }: Props) {
  const recipes = useRecipes();
  const projects = useProjectDirectory();
  const [page, setPage] = useState<HistoryCandidatePage | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [reload, setReload] = useState(0);
  const [picked, setPicked] = useState<number | null>(null);
  const [previewId, setPreviewId] = useState<number | null>(null);
  const [working, setWorking] = useState(false);
  const [range, setRange] = useState("all");
  const [project, setProject] = useState("");
  const [recipe, setRecipe] = useState("");
  const [sn, setSn] = useState("");
  const [groups, setGroups] = useState<string[]>([]);
  const [offset, setOffset] = useState(0);
  const currentId = projects.current?.id ?? null;
  const query = useMemo<HistoryCandidateQuery>(() => ({
    project: project === CURRENT_PROJECT ? currentId : project || null,
    recipeId: recipe || null,
    sn,
    from: rangeStart(ranges.find(r => r.value === range)!.days),
    verdicts: verdictGroups.filter(g => groups.includes(g.key)).flatMap(g => g.verdicts),
    offset,
    limit: PAGE,
  }), [project, currentId, recipe, sn, range, groups, offset]);
  // 选“当前项目”但当前项目还没读到时不查，免得把全部项目的记录当成当前项目的
  const blocked = project === CURRENT_PROJECT && !currentId;
  useEffect(() => {
    if (blocked) return;
    let alive = true;
    setLoadError("");
    setLoading(true);
    workspaceApi.historyCandidates(recipeId, query)
      .then(r => { if (alive) setPage(r); })
      .catch(e => { if (alive) { setPage({ total: 0, items: [] }); setLoadError(errorText(e)); } })
      .finally(() => { if (alive) setLoading(false); });
    return () => { alive = false; };
  }, [recipeId, query, blocked, reload]);
  useEffect(() => { setPicked(null); setPreviewId(null); }, [recipeId]);
  const rows = page?.items ?? null;
  const filter = (apply: () => void) => { apply(); setOffset(0); };
  const hasFilters = range !== "all" || !!project || !!recipe || !!sn || groups.length > 0;
  const reset = () => filter(() => { setRange("all"); setProject(""); setRecipe(""); setSn(""); setGroups([]); });
  const usable = rows?.filter(r => !blockers(r).length).length ?? 0;
  const pick = (id: number, on: boolean) => {
    setPicked(on ? id : null);
    if (on) setPreviewId(id);
  };
  const confirm = async () => {
    if (picked == null || working) return;
    setWorking(true);
    try {
      if (await onConfirm(picked)) { setPicked(null); setReload(n => n + 1); }
    } catch (e) { onError(errorText(e)); } finally { setWorking(false); }
  };
  const columns: TableColumn<HistoryCandidate>[] = [
    { key: "use", header: "选用", render: r => <span title={blockers(r).join("；") || undefined} onFocus={() => setPreviewId(r.recordId)}><Checkbox label={<span className="qs-sr-only">{"选用 SN " + r.sn}</span>} checked={r.recordId === picked} disabled={disabled || working || blockers(r).length > 0} onCheckedChange={on => pick(r.recordId, on)}/></span> },
    { key: "time", header: "时间", render: r => pieceTime(r.ts) },
    { key: "sn", header: "SN", render: r => <span className="mono">{r.sn}</span> },
    { key: "mode", header: "模式", render: r => <>{modeLabels[r.mode]}{r.fixture != null && <> <Badge tone="neutral">手动配置</Badge></>}</> },
    { key: "verdict", header: "判定", render: r => { const v = partVerdict(r); return <Badge tone={v.tone === "fault" ? "ng" : v.tone}>{v.label}</Badge>; } },
    { key: "owner", header: "项目 / 配方", render: r => <span className={r.recipeId === recipeId && r.projectId === currentId ? undefined : "muted"}>{r.projectId} · <span className="mono">{r.recipeId ?? "无配方"}</span></span> },
    { key: "groups", header: "帧组", render: r => r.groupCount },
    { key: "frames", header: "各相机帧数", render: r => r.cameras.map(c => `${nameOf(c.cameraId)} ${c.frames}`).join(" · ") || "—" },
    { key: "timing", header: "时长 / 实测帧率（设定）", render: timing },
    { key: "complete", header: "原图", render: r => <Badge tone={r.complete ? "ok" : "warn"}>{r.complete ? "完整" : "不完整"}</Badge> },
    { key: "reason", header: "不可用原因", render: r => <>{blockers(r).length ? blockers(r).join("；") : <span className="muted">可选用</span>}{r.warnings.map(w => <span key={w}> <Badge tone="warn">{w}</Badge></span>)}</> },
    { key: "preview", header: "原图预览", render: r => <Button size="sm" variant="text" aria-label={"预览 SN " + r.sn} aria-pressed={previewId === r.recordId} disabled={!r.groupCount} onClick={() => setPreviewId(r.recordId)}>看图</Button> },
  ];
  const preview = rows?.find(r => r.recordId === previewId) ?? null;
  return <Panel title={title} titleHelp={help} actions={page && <Badge tone={usable ? "ok" : "warn"}>{`本页可选用 ${usable} / ${page.items.length} 件，共 ${page.total} 件`}</Badge>}>
    <div className="wp-picker-filters" role="group" aria-label="筛选历史件">
      <SegmentedControl label="记录日期" options={ranges.map(({ value, label }) => ({ value, label }))} value={range} onValueChange={value => filter(() => setRange(value))} />
      <Select id="picker-project" aria-label="历史件项目" value={project} onValueChange={value => filter(() => setProject(value))}
        options={[
          { value: "", label: "全部项目" },
          { value: CURRENT_PROJECT, label: projects.current ? `当前项目（${projects.current.name}）` : "当前项目" },
          ...(projects.entries ?? []).filter(e => e.id !== currentId).map(e => ({ value: e.id, label: `${projectLabel(e)} · ${e.id}` })),
        ]} />
      <Select id="picker-recipe" aria-label="历史件配方" value={recipe} onValueChange={value => filter(() => setRecipe(value))}
        options={[
          { value: "", label: "全部配方" },
          { value: recipeId, label: `本配方 ${recipeId}` },
          ...recipes.filter(r => r.id !== recipeId).map(r => ({ value: r.id, label: r.id })),
        ]} />
      <TextField id="picker-sn" aria-label="历史件 SN" placeholder="筛选 SN" value={sn} onChange={e => { const value = e.target.value; filter(() => setSn(value)); }} />
      <div role="group" aria-label="按判定筛选">
        {verdictGroups.map(g => <Chip key={g.key} selected={groups.includes(g.key)} onClick={() => filter(() => setGroups(groups.includes(g.key) ? groups.filter(x => x !== g.key) : [...groups, g.key]))}>{g.label}</Chip>)}
      </div>
      {hasFilters && <Button variant="text" size="sm" onClick={reset}>重置筛选</Button>}
    </div>
    {loadError && <Notice title="读取历史件失败" tone="warn">{loadError}</Notice>}
    <div className="wp-picker-table">
      <Table caption="历史件" columns={columns} rows={rows ?? []} rowKey={r => String(r.recordId)}
        getRowProps={r => r.recordId === previewId ? { tone: "info" } : undefined}
        emptyState={<p className="muted center">{!rows || loading ? "正在读取历史件…" : hasFilters ? "没有符合筛选条件的录制件，试试放宽筛选。" : "本机还没有录制件。开启维护模式跟着生产采图后，可在这里选用。"}</p>}/>
    </div>
    {page && page.total > PAGE && <Pagination total={page.total} page={offset / PAGE + 1} pageSize={PAGE} pageSizeOptions={[PAGE]} disabled={loading}
      onPageChange={next => setOffset((next - 1) * PAGE)} />}
    <div className="wp-actions"><Button variant="filled" disabled={disabled || working || picked == null} onClick={() => void confirm()}>{confirmLabel}</Button></div>
    <aside className="wp-picker-preview" aria-label="历史件原图预览">
      {preview
        ? <FrameBrowser key={preview.recordId} source={String(preview.recordId)} label={`SN ${preview.sn}`} groupCount={preview.groupCount}
          cameras={preview.cameras.map(c => ({ id: c.cameraId, name: nameOf(c.cameraId) }))}
          read={(camera, seq, signal) => burstImageApi.record(preview.recordId, camera, seq, signal)} />
        : <p className="wp-capture-empty">勾选一件或点“看图”，在这里按相机、帧组查看录制原图。</p>}
    </aside>
  </Panel>;
}
