import { useCallback, useEffect, useRef, useState } from "react";
import { Alert, Button, Card, Grid, Heading, NumberField, Select, Stack, Text } from "qing-steel";
import { getAppInfo, getEngineStatus, openLogDir, type AppInfo, type EngineStatus } from "../lib/api";
import { cycleApi, devLyflowHint, useRecipes, type CycleSettings, type Timeouts } from "../features/cycle";
import { storageApi } from "../lib/storage";
import { errorText } from "../lib/errors";
import { useSnackbar } from "qing-steel";

const timeoutFields: [keyof Timeouts, string, string][] = [
  ["armMs", "布防预算（ms）", "布防及确认的时间预算，100–5000 ms"],
  ["motionMs", "运动超时（ms）", "等待运动结束"],
  ["drainMs", "收尾等待（ms）", "运动结束后等待剩余帧"],
  ["procMs", "单帧处理（ms）", "单帧入队到测量完成"],
  ["ackMs", "结果确认（ms）", "等待 PLC 确认结果"],
];
const timeoutMinimums: Record<keyof Timeouts, number> = { armMs: 100, motionMs: 1000, drainMs: 200, procMs: 200, ackMs: 500 };

type SavePart = (part: Partial<CycleSettings>) => Promise<boolean>;

function displayPath(path: string): string {
  if (/^\\\\\?\\UNC\\/i.test(path)) return `\\\\${path.slice(8)}`;
  if (/^\\\\\?\\[a-z]:\\/i.test(path)) return path.slice(4);
  return path;
}

function useMounted() {
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  return mounted;
}

/** 同页两个分区的读回与保存必须一起排队，避免同时读到旧设置后互相覆盖。 */
function useSavePart(): SavePart {
  const mounted = useMounted();
  const queue = useRef(Promise.resolve());
  return useCallback((part) => {
    const operation = queue.current.then(async () => {
      if (!mounted.current) return false;
      const cur = await cycleApi.getSettings();
      if (!mounted.current) return false;
      await cycleApi.saveSettings({ ...cur, ...part });
      return mounted.current;
    });
    queue.current = operation.then(() => undefined, () => undefined);
    return operation;
  }, [mounted]);
}

function useResource<T>(load: () => Promise<T>) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [generation, setGeneration] = useState(0);
  const reload = useCallback(() => setGeneration((value) => value + 1), []);
  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    Promise.resolve().then(load).then((value) => { if (active) setData(value); })
      .catch((error: unknown) => { if (active) setError(errorText(error)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [load, generation]);
  return { data, setData, error, loading, reload };
}

function integerIn(value: number, min: number, max = Number.MAX_SAFE_INTEGER) {
  return Number.isSafeInteger(value) && value >= min && value <= max;
}

function timingError(settings: CycleSettings) {
  if (!integerIn(settings.historyDays, 1, 3650)) return "记录保留天数需为 1–3650 之间的整数";
  for (const [key, label] of timeoutFields) {
    if (!integerIn(settings.timeouts[key], timeoutMinimums[key])) return `${label}需为不小于 ${timeoutMinimums[key]} 的整数`;
  }
  return null;
}

function measureError(settings: CycleSettings) {
  if (!integerIn(settings.recordKeep, 1, 100000)) return "帧录制保留件数需为 1–100000 之间的整数";
  if (!Number.isFinite(settings.recordMaxGb) || settings.recordMaxGb < 0.5 || settings.recordMaxGb > 10000)
    return "帧录制总大小需在 0.5–10000 GB 之间";
  if (!integerIn(settings.offlineRecordKeep, 1, 100000)) return "离线原图保留件数需为 1–100000 之间的整数";
  if (!Number.isFinite(settings.offlineRecordMaxGb) || settings.offlineRecordMaxGb < 0.5 || settings.offlineRecordMaxGb > 10000)
    return "离线原图总大小需在 0.5–10000 GB 之间";
  return null;
}

function numberValue(value: number | "") { return value === "" ? Number.NaN : value; }
function numberInput(value: number) { return Number.isNaN(value) ? "" : value; }

function CycleSettingsPanel({ savePart }: { savePart: SavePart }) {
  const recipes = useRecipes();
  const { data: settings, setData: setSettings, error, loading, reload } = useResource(cycleApi.getSettings);
  const [saveError, setSaveError] = useState("");
  const { show: snack } = useSnackbar();
  const [saving, setSaving] = useState(false);
  const pending = useRef(false);
  const mounted = useMounted();
  const invalid = settings ? timingError(settings) : null;
  const update = (next: CycleSettings) => { setSettings(next); setSaveError(""); };

  const save = async () => {
    if (!settings || pending.current || invalid) return;
    pending.current = true;
    setSaving(true);
    setSaveError("");
    try {
      const { productSource, manualRecipeId, historyDays, timeouts } = settings;
      const saved = await savePart({ productSource, manualRecipeId, historyDays, timeouts });
      if (saved && mounted.current) snack("检测节拍已保存，从下一个工件开始生效");
    } catch (e) {
      if (mounted.current) setSaveError(errorText(e));
    } finally {
      pending.current = false;
      if (mounted.current) setSaving(false);
    }
  };

  return (
    <Card role="region" aria-label="检测节拍" title={<Heading level={3}>检测节拍</Heading>}
      actions={<Button variant="filled" disabled={!settings || saving || !!invalid} aria-busy={saving} onClick={save}>{saving ? "保存中…" : "保存"}</Button>}>
      <Stack direction="column" align="stretch" gap={12} wrap={false}>
      {loading && !settings && <p role="status">正在读取检测节拍设置…</p>}
      {error && <Alert tone="ng" action={<Button size="sm" onClick={reload}>重试</Button>}>{error}</Alert>}
      {settings && <>
      <fieldset disabled={saving} style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}>
        <Grid minWidth={220} gap={14}>
          <Select
            id="product-source"
            label="型号来源"
            value={settings.productSource}
            onValueChange={(value) => update({ ...settings, productSource: value as CycleSettings["productSource"] })}
            options={[{ value: "plc", label: "PLC 下发型号" }, { value: "manual", label: "人工选择配方" }]}
          />
          {settings.productSource === "manual" && (
            <Select
              id="manual-recipe-setting"
              label="当前配方"
              value={settings.manualRecipeId ?? ""}
              onValueChange={(value) => update({ ...settings, manualRecipeId: value || null })}
              options={[{ value: "", label: "未选择" }, ...recipes.map((r) => ({ value: r.id, label: `${r.id} · ${r.name}` }))]}
            />
          )}
          <NumberField
            id="history-days"
            label="记录保留（天）"
            description="超过天数的检测记录每天自动删除"
            min={1}
            max={3650}
            value={numberInput(settings.historyDays)}
            onValueChange={(value) => update({ ...settings, historyDays: numberValue(value) })}
          />
          {timeoutFields.map(([key, label, hint]) => (
            <NumberField
              key={key}
              id={`timeout-${key}`}
              label={label}
              description={hint}
              min={timeoutMinimums[key]}
              step={1}
              value={numberInput(settings.timeouts[key])}
              onValueChange={(value) => update({ ...settings, timeouts: { ...settings.timeouts, [key]: numberValue(value) } })}
            />
          ))}
        </Grid>
      </fieldset>
      <Text as="p" variant="label" muted>
        PLC 下发：按型号匹配配方，匹配不到判 ERR 95。人工选择：操作员在实时检测页切换配方，仅空闲时可切换，不读取型号。
      </Text>
      <details>
        <summary>查看节拍协议说明</summary>
        <Text as="p" variant="label" muted>布防预算从读到 plcState=10 起，到 pcState=20 读回确认为止；回写前超时拒收本件（90/99），回写确认后超时进入视觉故障。运动超时从 pcState=20 起等待 plcState=20；收尾等待从 plcState=20 起等待剩余帧；结果确认从 pcState=30 起等待 plcState=30。</Text>
      </details>
      {invalid && <Alert tone="ng">{invalid}</Alert>}
      </>}
      {saveError && <Alert tone="ng">{saveError}</Alert>}
      </Stack>
    </Card>
  );
}

function MeasurePanel({ savePart }: { savePart: SavePart }) {
  const { data: settings, setData: setSettings, error, loading, reload } = useResource(cycleApi.getSettings);
  const { data: engine, error: engineError, loading: engineLoading, reload: refresh } = useResource<EngineStatus>(getEngineStatus);
  const [saveError, setSaveError] = useState("");
  const { show: snack } = useSnackbar();
  const [saving, setSaving] = useState(false);
  const pending = useRef(false);
  const mounted = useMounted();
  const invalid = settings ? measureError(settings) : null;
  const visibleEngine = !engineLoading && !engineError ? engine : null;
  const update = (next: CycleSettings) => { setSettings(next); setSaveError(""); };

  const save = async () => {
    if (!settings || pending.current || invalid) return;
    pending.current = true;
    setSaving(true);
    setSaveError("");
    try {
      const { record, recordKeep, recordMaxGb, offlineRecordKeep, offlineRecordMaxGb } = settings;
      const saved = await savePart({ record, recordKeep, recordMaxGb, offlineRecordKeep, offlineRecordMaxGb });
      if (saved && mounted.current) {
        snack("lyFlow 与帧录制设置已保存");
        refresh();
      }
    } catch (e) {
      if (mounted.current) setSaveError(errorText(e));
    } finally {
      pending.current = false;
      if (mounted.current) setSaving(false);
    }
  };

  return (
    <Card role="region" aria-label="lyFlow 与帧录制" title={<Heading level={3}>lyFlow 与帧录制</Heading>}
      actions={<Button variant="filled" disabled={!settings || saving || !!invalid} aria-busy={saving} onClick={save}>{saving ? "保存中…" : "保存"}</Button>}>
      <Stack direction="column" align="stretch" gap={12} wrap={false}>
      {loading && !settings && <p role="status">正在读取 lyFlow 与帧录制设置…</p>}
      {error && <Alert tone="ng" action={<Button size="sm" onClick={reload}>重试</Button>}>{error}</Alert>}
      {settings && <>
      <fieldset disabled={saving} style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}>
        <Grid minWidth={220} gap={14}>
          <Select id="record-mode" label="真机帧录制" value={settings.record} onValueChange={(value) => update({ ...settings, record: value as CycleSettings["record"] })}
            options={[{ value: "off", label: "关闭" }, { value: "failed", label: "只留 NG / ERR 件" }, { value: "all", label: "全部" }]} />
          <NumberField
            id="record-keep"
            label="真机录制最多保留（件）"
            min={1}
            max={100000}
            value={numberInput(settings.recordKeep)}
            onValueChange={(value) => update({ ...settings, recordKeep: numberValue(value) })}
          />
          <NumberField
            id="record-max-gb"
            label="真机录制总大小上限（GB）"
            min={0.5}
            max={10000}
            step={0.5}
            value={numberInput(settings.recordMaxGb)}
            onValueChange={(value) => update({ ...settings, recordMaxGb: numberValue(value) })}
          />
          <NumberField
            id="offline-record-keep"
            label="离线原图最多保留（件）"
            min={1}
            max={100000}
            step={1}
            value={numberInput(settings.offlineRecordKeep)}
            onValueChange={(value) => update({ ...settings, offlineRecordKeep: numberValue(value) })}
          />
          <NumberField
            id="offline-record-max-gb"
            label="离线原图总大小上限（GB）"
            min={0.5}
            max={10000}
            step={0.5}
            value={numberInput(settings.offlineRecordMaxGb)}
            onValueChange={(value) => update({ ...settings, offlineRecordMaxGb: numberValue(value) })}
          />
        </Grid>
      </fieldset>
      <Text as="p" variant="label" muted>离线原图超出件数或容量上限时，后续清理会删除最旧的离线原图，不影响导入的样本集。退出离线后恢复真机帧录制设置。</Text>
      {invalid && <Alert tone="ng">{invalid}</Alert>}
      </>}
      <dl className="kv">
        <dt>引擎</dt>
        <dd className={visibleEngine?.ready ? "ok" : "muted"}>
          {engineLoading ? "正在读取…" : visibleEngine ? `${visibleEngine.backend} · ${visibleEngine.ready ? "就绪" : "未就绪"}${visibleEngine.version ? ` · ${visibleEngine.version}` : ""}` : "--"}
        </dd>
        <dt>说明</dt>
        <dd>{visibleEngine?.message ?? "--"}</dd>
        <dt>运行库目录</dt>
        <dd className="mono">{displayPath(visibleEngine?.path ?? "--")}</dd>
      </dl>
      {import.meta.env.DEV && visibleEngine && !visibleEngine.ready && <Alert tone="info">{devLyflowHint}</Alert>}
      {engineError && <Alert tone="ng" action={<Button size="sm" onClick={refresh}>重试引擎状态</Button>}>引擎状态读取失败：{engineError}</Alert>}
      <Text as="p" variant="label" muted>
        真机和离线样本集使用 lyFlow，离线合成图使用模拟测量。帧录制的原图可用于历史复测。
      </Text>
      {saveError && <Alert tone="ng">{saveError}</Alert>}
      </Stack>
    </Card>
  );
}

function StoragePanel() {
  const { data, error, loading, reload } = useResource(storageApi.status);
  const [openError, setOpenError] = useState("");
  const openDirectory = (target: "data" | "runtime" | "vision") => {
    setOpenError(""); void storageApi.openDirectory(target).catch(e => setOpenError(errorText(e)));
  };
  const environment: Record<string, string> = { development: "本地开发", production: "正式版", test: "自动化测试" };
  const migration: Record<string, string> = { fresh: "新资料目录", completed: "迁移完成", migrated: "迁移完成", none: "无需迁移", ready: "资料目录已启用", pending: "等待迁移", failed: "迁移失败" };
  return <Card role="region" aria-label="资料存储" title={<Heading level={3}>资料存储</Heading>}
    actions={<>
      <Button disabled={!data || loading || !!error} onClick={() => openDirectory("data")}>打开资料目录</Button>
      <Button disabled={!data?.runtimeLogDir || loading || !!error} onClick={() => openDirectory("runtime")}>打开运行日志</Button>
      <Button disabled={!data?.visionLogDir || loading || !!error} onClick={() => openDirectory("vision")}>打开视觉原图</Button>
    </>}>
    <Stack direction="column" align="stretch" gap={12} wrap={false}>
    {loading && <p role="status">正在读取资料目录…</p>}
    {error && <Alert tone="ng" action={<Button size="sm" onClick={reload}>重试</Button>}>资料目录读取失败：{error}</Alert>}
    {!loading && !error && data && <>
      <dl className="kv"><dt>当前环境</dt><dd>{environment[data.environment] ?? data.environment}</dd>
        <dt>资料目录</dt><dd className="mono" style={{ overflowWrap: "anywhere" }}>{displayPath(data.root)}</dd>
        <dt>日志根目录</dt><dd className="mono" style={{ overflowWrap: "anywhere" }}>{displayPath(data.logRoot ?? "—")}</dd>
        <dt>运行日志</dt><dd className="mono" style={{ overflowWrap: "anywhere" }}>{displayPath(data.runtimeLogDir ?? "—")}</dd>
        <dt>当前项目视觉原图</dt><dd className="mono" style={{ overflowWrap: "anywhere" }}>{displayPath(data.visionLogDir ?? "—")}</dd>
        <dt>迁移状态</dt><dd>{migration[data.migrationStatus] ?? data.migrationStatus}</dd>
      </dl>
      {data.migrationMessage && <p role="status">{data.migrationMessage}</p>}
    </>}
    <Text as="p" variant="label" muted>项目资料与日志分别存放，卸载默认保留。示教和标定原图随项目保存。</Text>
    {openError && <Alert tone="ng">打开目录失败：{openError}</Alert>}
    </Stack>
  </Card>;
}

export default function SettingsPage() {
  const { data: info, error, loading, reload } = useResource<AppInfo>(getAppInfo);
  const savePart = useSavePart();
  const [logError, setLogError] = useState<string | null>(null);
  const openLogs = () => {
    setLogError(null);
    openLogDir().catch((e: unknown) => setLogError(errorText(e)));
  };

  return (
    <Stack direction="column" align="stretch" gap={16} wrap={false}>
      <StoragePanel />
      <CycleSettingsPanel savePart={savePart} />
      <MeasurePanel savePart={savePart} />
      <Card role="region" aria-label="关于" title={<Heading level={3}>关于</Heading>}>
        <Stack direction="column" align="stretch" gap={12} wrap={false}>
        {loading && <p role="status">正在读取应用信息…</p>}
        {error && <Alert tone="ng" action={<Button size="sm" onClick={reload}>重试应用信息</Button>}>{error}</Alert>}
        <dl className="kv">
          <dt>应用</dt>
          <dd>{info?.name ?? "--"}</dd>
          <dt>版本</dt>
          <dd>{info?.version ?? "--"}</dd>
          <dt>构建</dt>
          <dd className="mono">{info?.gitSha ? `${info.gitSha}${info.buildTime ? ` · ${info.buildTime}` : ""}` : "--"}</dd>
          <dt>日志目录</dt>
          <dd className="mono">{displayPath(info?.logDir ?? "--")}</dd>
        </dl>
        <div><Button onClick={openLogs}>打开日志目录</Button></div>
        {logError && <Alert tone="ng">打开日志目录失败：{logError}</Alert>}
        </Stack>
      </Card>
    </Stack>
  );
}
