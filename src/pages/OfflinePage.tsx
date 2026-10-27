import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { FolderOpen, LogIn, LogOut, OctagonX, Play, Repeat, Square, Zap } from "lucide-react";
import { Alert, Button, Card, Heading, Select, Stack, Table, Text, type TableColumn } from "qing-steel";
import { usePlcStatus } from "../features/plc";
import { formatTime, sampleLabelText, verdictLabel } from "../features/history/meta";
import { errorText } from "../lib/errors";
import {
  cycleApi,
  faultOptions,
  measurementLabel,
  ReadinessChecklist,
  RecipeRunSelect,
  rejectionTarget,
  scenariosFor,
  showChecklist,
  siteApi,
  sourceLabel,
  TargetLink,
  useReadiness,
  useRecipes,
  useRunChoice,
  useSiteStatus,
  type OfflineChoice,
  type OfflineFault,
  type ReadinessTarget,
  type SampleLabel,
  type SampleRun,
  type SampleSetSummary,
  type Scenario,
  type SiteSourceKind,
  type SiteStatus,
} from "../features/cycle";
import SamplePartSelect from "../features/cycle/components/SamplePartSelect";

const purposeText: Record<SampleRun["purpose"], string> = { production: "已发布配方离线回放", candidateDebug: "候选调试" };
const labels: SampleLabel[] = ["ok", "ng", "unknown"];

const recentColumns: TableColumn<SampleRun>[] = [
  { key: "ts", header: "时间", render: r => <span className="mono nowrap">{formatTime(r.ts)}</span> },
  { key: "partId", header: "样本件", render: r => <span className="mono">{r.partId}</span> },
  { key: "sampleSn", header: "样本 SN", render: r => <span className="sn">{r.sampleSn}</span> },
  { key: "partSn", header: "PLC SN", render: r => <span className="sn">{r.partSn}</span> },
  { key: "purpose", header: "用途", render: r => purposeText[r.purpose] },
  { key: "label", header: "期望", render: r => sampleLabelText[r.label] },
  { key: "verdict", header: "实际", render: r => r.verdict ? verdictLabel[r.verdict] : r.message || "—" },
];

function useOperation() {
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<{ text: string; target: ReadinessTarget | null } | null>(null);
  const pending = useRef(false);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);
  const perform = async (operation: () => Promise<unknown>, locate?: () => Promise<ReadinessTarget | null>) => {
    if (pending.current) return;
    pending.current = true; setBusy(true); setFailure(null);
    try { await operation(); } catch (e) {
      const text = errorText(e);
      const target = locate ? await locate() : null;
      if (alive.current) setFailure({ text, target });
    }
    finally { pending.current = false; if (alive.current) setBusy(false); }
  };
  return { busy, error: failure?.text ?? "", failure, perform, alive };
}

/** 后端样本集摘要只有相机 ID 与视角（不带每台相机的帧数）；多视角是旧拍照点模型（三目拼接）的样本集，仍标出以免与段配方的单视角相机混淆。 */
function cameraText(camera: SampleSetSummary["cameras"][number]) {
  const legacy = camera.views.length > 1 ? `（旧模型 ${camera.views.length} 视角）` : "";
  return `${camera.id}${legacy}`;
}

function SampleSetView({ sampleSet }: { sampleSet: SampleSetSummary }) {
  const missingTitle = sampleSet.missingCount > 0 ? sampleSet.missing.slice(0, 10).join("\n") + (sampleSet.missingCount > 10 ? "\n…" : "") : undefined;
  return (
    <dl className="kv offline-summary" aria-label="样本集摘要">
      <dt>名称</dt><dd>{sampleSet.name}</dd>
      <dt>目录</dt><dd className="mono">{sampleSet.dir}</dd>
      <dt>件数</dt><dd>{sampleSet.parts.length}</dd>
      <dt>每件帧数</dt><dd>{sampleSet.shotCount}</dd>
      <dt>缺图</dt><dd className={sampleSet.missingCount > 0 ? "c-ng" : undefined} title={missingTitle}>{sampleSet.missingCount}</dd>
      <dt>标签</dt><dd>{labels.map(l => `${sampleLabelText[l]} ${sampleSet.parts.filter(p => p.label === l).length}`).join(" · ")}</dd>
      <dt>相机</dt><dd>{sampleSet.cameras.map(cameraText).join("、") || "—"}</dd>
    </dl>
  );
}

function OfflineEntry({ last }: { last: OfflineChoice | null }) {
  const [kind, setKind] = useState<SiteSourceKind>(last?.kind ?? "sampleSet");
  const [dir, setDir] = useState(last?.sampleSetDir ?? "");
  const [preview, setPreview] = useState<SampleSetSummary | null>(null);
  const [previewError, setPreviewError] = useState("");
  const [loading, setLoading] = useState(false);
  const serial = useRef(0);
  const previousChoice = useRef<{ dir: string; preview: SampleSetSummary | null }>({ dir, preview: null });
  const { busy, error, perform, alive } = useOperation();

  const load = async (target: string) => {
    const request = ++serial.current;
    setPreviewError(""); setLoading(true);
    try {
      const summary = await siteApi.previewSampleSet(target);
      if (alive.current && request === serial.current) { setDir(target); setPreview(summary); }
    } catch (e) {
      if (alive.current && request === serial.current) setPreviewError(errorText(e));
    } finally {
      if (alive.current && request === serial.current) setLoading(false);
    }
  };
  useEffect(() => {
    if (last?.sampleSetDir) void load(last.sampleSetDir);
    return () => { serial.current++; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const choose = () => void perform(async () => {
    const picked = await siteApi.pickDir(dir, "选择离线样本集目录");
    if (picked !== null && alive.current) {
      previousChoice.current = { dir, preview: preview?.managedDir ? null : preview };
      await load(picked);
    }
  });
  const enter = () => void perform(() => siteApi.enterOffline(kind === "sampleSet" ? { kind, dir } : { kind }));
  const ready = kind === "synthetic" || (!!preview && !preview.managedDir && !loading);

  return (
    <Card title={<Heading level={3}>当前：真机</Heading>}>
      <Stack direction="column" align="stretch" gap={12}>
      <Select label="离线来源" value={kind} onValueChange={(value) => setKind(value as SiteSourceKind)} disabled={busy}
        options={[{ value: "sampleSet", label: "样本集（现场原图）" }, { value: "synthetic", label: "合成图" }]} />
      {kind === "sampleSet" && <>
        <Stack gap={8}>
          <Button leadingIcon={<FolderOpen size={15} />} onClick={choose} disabled={busy || loading}>选择样本集目录</Button>
          <Text mono muted>{dir || "未选择"}</Text>
        </Stack>
        {loading && <p className="muted" role="status">正在读取样本集…</p>}
        {previewError && <Alert tone="ng">{previewError}</Alert>}
        {preview && <SampleSetView sampleSet={preview} />}
        {preview?.managedDir && <Alert tone="warn" action={<>
          <Button disabled={busy || loading} onClick={() => void load(preview.managedDir!)}>使用已有副本</Button>
          <Button disabled={busy || loading} onClick={() => {
            serial.current++; setDir(previousChoice.current.dir); setPreview(previousChoice.current.preview); setPreviewError("");
          }}>取消本次选择</Button>
        </>}>
          <p>此样本集 ID 已导入。请选择已有副本，或取消本次选择；不会覆盖已保存的样本。</p>
          <p className="mono">{preview.managedDir}</p>
        </Alert>}
        <Text as="p" variant="label" muted>进入离线时会将外部样本复制到资料目录，保留源文件；之后使用已导入副本。</Text>
      </>}
      <Alert title="离线模式">
        使用{sourceLabel[kind]}和进程内 S7 服务，{kind === "sampleSet" ? "通过 lyFlow 测量" : "使用模拟测量"}。离线帧录制保留全部，候选调试引用样本原图。离线期间 PLC 与相机组配置只读，退出后恢复真机配置。
      </Alert>
      <div>
        <Button variant="filled" leadingIcon={<LogIn size={15} />} onClick={enter} disabled={busy || !ready}>进入离线</Button>
      </div>
      {error && <Alert tone="ng">{error}</Alert>}
      </Stack>
    </Card>
  );
}

function OfflineSession({ status }: { status: SiteStatus }) {
  const { readiness, refresh } = useReadiness(`${status.source ?? ""}:${status.sampleSet?.id ?? ""}`);
  const options = readiness?.recipes.filter(recipe => recipe.published) ?? null;
  const summaries = useRecipes();
  const plc = usePlcStatus();
  const [scenario, setScenario] = useState<Scenario>("normal");
  const [fault, setFault] = useState<OfflineFault>("disconnect");
  const [partId, setPartId] = useState("");
  const [stopRequested, setStopRequested] = useState(false);
  const { busy, failure, perform, alive } = useOperation();
  const running = status.running;
  const [recipeId, setRecipeId] = useRunChoice(options, running);
  const sampleSet = status.source === "sampleSet" ? status.sampleSet : null;
  const scenarios = scenariosFor(status.source);
  const chosen = scenarios.some(([v]) => v === scenario) ? scenario : "normal";

  useEffect(() => {
    if (!running) setStopRequested(false);
  }, [running]);
  useEffect(() => {
    if (partId && !sampleSet?.parts.some(p => p.partId === partId)) setPartId("");
  }, [sampleSet, partId]);

  const connected = plc?.state === "connected";
  const canRun = !running && !busy && connected && !!recipeId;
  const run = (continuous: boolean) => void perform(() => cycleApi.offlineRun(recipeId, chosen, continuous, continuous || !partId ? null : partId),
    async () => rejectionTarget((await refresh()) ?? readiness, recipeId));
  const message = !connected ? "等待连接离线 S7 服务…" : stopRequested && running ? "已请求停止，等待本件完成…" : status.message;

  return (
    <Stack direction="column" align="stretch" gap={12}>
      {failure && <Alert tone="ng">{failure.text} <TargetLink target={failure.target} /></Alert>}
      <Card role="region" aria-label="离线状态" title={<Heading level={3}>当前：离线 · {status.source ? sourceLabel[status.source] : "—"}</Heading>}
        actions={<Button leadingIcon={<LogOut size={15} />} onClick={() => void perform(siteApi.leaveOffline)} disabled={busy || running}>退出离线</Button>}>
        <dl className="kv offline-summary">
          <dt>S7 服务</dt><dd className="mono">127.0.0.1:{status.port ?? "—"}</dd>
          <dt>测量方式</dt><dd>{measurementLabel[status.measurement]}</dd>
          <dt>已运行</dt><dd>{status.parts} 件</dd>
        </dl>
        {sampleSet && <SampleSetView sampleSet={sampleSet} />}
      </Card>
      {sampleSet && <Card role="region" aria-label="候选配方离线试运行" title={<Heading level={3}>候选配方离线试运行 · 无需发布</Heading>}>
        <Stack direction="column" align="start" gap={12}>
          <p className="muted">在在线检测页面选择已保存候选和样本件，测试整圈流程。结果标记为调试，不替代标定、独立样本验证或生产发布。</p>
          <Link className="qs-btn tonal" to="/inspect">前往在线检测调试候选</Link>
        </Stack>
      </Card>}
      <Card role="region" aria-label="已发布配方离线回放" title={<Heading level={3}>已发布配方离线回放</Heading>}>
        <Stack direction="column" align="stretch" gap={12}>
          <p className="muted">下列检查与启动操作仅用于已发布版本的离线回放；候选调试不要求发布，启动时单独检查示教和样本匹配。</p>
          {showChecklist(readiness, running) && <ReadinessChecklist readiness={readiness} embedded />}
          <Stack gap={8}>
            <RecipeRunSelect options={options} summaries={summaries} value={recipeId} onChange={setRecipeId} disabled={running || busy} />
            <Select aria-label="离线工况" value={chosen} onValueChange={(value) => setScenario(value as Scenario)} disabled={running || busy}
              options={scenarios.map(([v, label]) => ({ value: v, label }))} />
            {sampleSet && <SamplePartSelect sampleSet={sampleSet} nextPart={status.nextPart} value={partId} onChange={setPartId} disabled={running || busy} />}
            <Button variant="filled" leadingIcon={<Play size={15} />} onClick={() => run(false)} disabled={!canRun}>运行一件</Button>
            <Button leadingIcon={<Repeat size={15} />} onClick={() => run(true)} disabled={!canRun}>连续运行</Button>
          </Stack>
        </Stack>
      </Card>
      <Card role="region" aria-label="当前离线运行" title={<Heading level={3}>当前离线运行</Heading>}>
        <Stack direction="column" align="stretch" gap={12}>
          <p className="muted">停止当前离线任务，包括候选调试或已发布配方回放。</p>
          <Stack gap={8}>
            <Button leadingIcon={<Square size={15} />} onClick={() => void perform(async () => {
              await cycleApi.offlineStop(false);
              if (alive.current) setStopRequested(true);
            })} disabled={busy || stopRequested || !running || !status.continuous}>本件后停止</Button>
            <Button danger leadingIcon={<OctagonX size={15} />} onClick={() => void perform(() => cycleApi.offlineStop(true))} disabled={busy || !running}>立即停止</Button>
          </Stack>
          <Text as="p" variant="label" muted role="status">{message}</Text>
        </Stack>
      </Card>
      <Card title={<Heading level={3}>故障注入</Heading>}>
        <Stack gap={8}>
          <Select aria-label="故障注入" value={fault} onValueChange={(value) => setFault(value as OfflineFault)} disabled={busy}
            options={faultOptions.map(([v, label]) => ({ value: v, label }))} />
          <Button leadingIcon={<Zap size={15} />} onClick={() => void perform(() => cycleApi.offlineInject(fault))} disabled={busy}>注入故障</Button>
          {status.heartbeatStopped && <Text variant="label">PLC 心跳已停止</Text>}
        </Stack>
      </Card>
      {status.recent.length > 0 && (
        <Card title={<Heading level={3}>最近样本件</Heading>}>
          <Table<SampleRun> caption="最近样本件" rows={status.recent} columns={recentColumns} rowKey={(r) => `${r.ts}:${r.partSn}:${r.partId}`} />
        </Card>
      )}
    </Stack>
  );
}

export default function OfflinePage() {
  const status = useSiteStatus();
  if (!status) return <p role="status" className="muted">正在读取站点模式…</p>;
  return status.mode === "offline" ? <OfflineSession status={status} /> : <OfflineEntry last={status.last} />;
}
