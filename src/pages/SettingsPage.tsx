import { useCallback, useEffect, useRef, useState } from "react";
import { getAppInfo, getEngineStatus, type AppInfo, type EngineStatus } from "../lib/api";
import { cycleApi, useRecipes, type CycleSettings, type Timeouts } from "../features/cycle";

const timeoutFields: [keyof Timeouts, string, string][] = [
  ["armMs", "布防目标 T_arm（ms）", "partStart↑ → armed↑，超出仅报警"],
  ["motionMs", "运动超时 T_motion（ms）", "armed↑ 后等待 partEnd↑"],
  ["drainMs", "收尾等待 T_drain（ms）", "partEnd↑ 后等待剩余帧"],
  ["procMs", "单帧处理 T_proc（ms）", "单帧入队到测量完成"],
  ["ackMs", "结果确认 T_ack（ms）", "done↑ 后等待 resultAck↑"],
];
const timeoutMinimums: Record<keyof Timeouts, number> = { armMs: 0, motionMs: 1000, drainMs: 200, procMs: 200, ackMs: 500 };

type SavePart = (part: Partial<CycleSettings>) => Promise<boolean>;

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
      .catch((error: unknown) => { if (active) setError(String(error)); })
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
  return null;
}

function numberValue(value: string) { return value === "" ? Number.NaN : Number(value); }
function numberInput(value: number) { return Number.isNaN(value) ? "" : value; }

function CycleSettingsPanel({ savePart }: { savePart: SavePart }) {
  const recipes = useRecipes();
  const { data: settings, setData: setSettings, error, loading, reload } = useResource(cycleApi.getSettings);
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const pending = useRef(false);
  const mounted = useMounted();
  const invalid = settings ? timingError(settings) : null;
  const update = (next: CycleSettings) => { setSettings(next); setNotice(null); };

  const save = async () => {
    if (!settings || pending.current || invalid) return;
    pending.current = true;
    setSaving(true);
    setNotice(null);
    try {
      const { productSource, manualRecipeId, historyDays, timeouts } = settings;
      const saved = await savePart({ productSource, manualRecipeId, historyDays, timeouts });
      if (saved && mounted.current) setNotice({ ok: true, text: "已保存，从下一个工件开始生效" });
    } catch (e) {
      if (mounted.current) setNotice({ ok: false, text: String(e) });
    } finally {
      pending.current = false;
      if (mounted.current) setSaving(false);
    }
  };

  return (
    <div className="panel">
      <div className="panel-toolbar">
        <h3 className="panel-title">检测节拍</h3>
        <button className="btn primary" disabled={!settings || saving || !!invalid} aria-busy={saving} onClick={save}>{saving ? "保存中…" : "保存"}</button>
      </div>
      {loading && !settings && <p role="status">正在读取检测节拍设置…</p>}
      {error && <div className="notice error" role="alert">{error} <button className="btn" onClick={reload}>重试</button></div>}
      {settings && <>
      <fieldset className="form-grid" disabled={saving} style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}>
        <label className="field">
          <span>型号来源</span>
          <select
            id="product-source"
            className="input"
            value={settings.productSource}
            onChange={(e) => update({ ...settings, productSource: e.target.value as CycleSettings["productSource"] })}
          >
            <option value="plc">PLC 下发产品代码</option>
            <option value="manual">人工选择配方</option>
          </select>
        </label>
        {settings.productSource === "manual" && (
          <label className="field">
            <span>当前配方</span>
            <select
              id="manual-recipe-setting"
              className="input"
              value={settings.manualRecipeId ?? ""}
              onChange={(e) => update({ ...settings, manualRecipeId: e.target.value || null })}
            >
              <option value="">未选择</option>
              {recipes.map((r) => (
                <option key={r.id} value={r.id}>
                  {r.id} · {r.name}
                </option>
              ))}
            </select>
          </label>
        )}
        <label className="field" title="超过天数的检测记录每天自动删除">
          <span>记录保留（天）</span>
          <input
            id="history-days"
            className="input mono"
            type="number"
            min={1}
            max={3650}
            value={numberInput(settings.historyDays)}
            onChange={(e) => update({ ...settings, historyDays: numberValue(e.target.value) })}
          />
        </label>
        {timeoutFields.map(([key, label, hint]) => (
          <label key={key} className="field" title={hint}>
            <span>{label}</span>
            <input
              id={`timeout-${key}`}
              className="input mono"
              type="number"
              min={timeoutMinimums[key]}
              step={1}
              value={numberInput(settings.timeouts[key])}
              onChange={(e) => update({ ...settings, timeouts: { ...settings.timeouts, [key]: numberValue(e.target.value) } })}
            />
          </label>
        ))}
      </fieldset>
      <p className="muted hint">
        PLC 下发：按产品代码匹配配方，匹配不到判 ERR 95。人工选择：操作员在实时检测页切换配方，仅空闲时可切换，不读取产品代码。
      </p>
      {invalid && <div className="notice error" role="alert">{invalid}</div>}
      </>}
      {notice && <div className={`notice ${notice.ok ? "ok" : "error"}`} role={notice.ok ? "status" : "alert"}>{notice.text}</div>}
    </div>
  );
}

function MeasurePanel({ savePart }: { savePart: SavePart }) {
  const { data: settings, setData: setSettings, error, loading, reload } = useResource(cycleApi.getSettings);
  const { data: engine, error: engineError, loading: engineLoading, reload: refresh } = useResource<EngineStatus>(getEngineStatus);
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const pending = useRef(false);
  const mounted = useMounted();
  const invalid = settings ? measureError(settings) : null;
  const visibleEngine = !engineLoading && !engineError ? engine : null;
  const update = (next: CycleSettings) => { setSettings(next); setNotice(null); };

  const save = async () => {
    if (!settings || pending.current || invalid) return;
    pending.current = true;
    setSaving(true);
    setNotice(null);
    try {
      const { lyflowCore, vision, record, recordKeep, recordMaxGb } = settings;
      const saved = await savePart({ lyflowCore, vision, record, recordKeep, recordMaxGb });
      if (saved && mounted.current) {
        setNotice({ ok: true, text: "已保存" });
        refresh();
      }
    } catch (e) {
      if (mounted.current) setNotice({ ok: false, text: String(e) });
    } finally {
      pending.current = false;
      if (mounted.current) setSaving(false);
    }
  };

  return (
    <div className="panel">
      <div className="panel-toolbar">
        <h3 className="panel-title">测量与帧录制</h3>
        <button className="btn primary" disabled={!settings || saving || !!invalid} aria-busy={saving} onClick={save}>{saving ? "保存中…" : "保存"}</button>
      </div>
      {loading && !settings && <p role="status">正在读取测量与帧录制设置…</p>}
      {error && <div className="notice error" role="alert">{error} <button className="btn" onClick={reload}>重试</button></div>}
      {settings && <>
      <fieldset className="form-grid" disabled={saving} style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}>
        <label className="field">
          <span>飞拍配方</span>
          <select id="measure-fly" className="input" value={settings.vision ? "lyFlow" : "sim"} onChange={(e) => update({ ...settings, vision: e.target.value === "lyFlow" })}>
            <option value="sim">模拟测量（不看图像）</option>
            <option value="lyFlow">lyFlow 流程（沿示教中线量胶）</option>
          </select>
        </label>
        {settings.vision && (
          <label className="field span-2">
            <span>核心库路径（lyflow_core.dll）</span>
            <input
              id="lyflow-core"
              className="input mono"
              placeholder="留空自动使用安装包内置引擎；也可指定其他核心库路径"
              value={settings.lyflowCore ?? ""}
              onChange={(e) => update({ ...settings, lyflowCore: e.target.value || null })}
            />
          </label>
        )}
        <label className="field">
          <span>帧录制</span>
          <select id="record-mode" className="input" value={settings.record} onChange={(e) => update({ ...settings, record: e.target.value as CycleSettings["record"] })}>
            <option value="off">关闭</option>
            <option value="failed">只留 NG / ERR 件</option>
            <option value="all">全部</option>
          </select>
        </label>
        <label className="field">
          <span>录制最多保留（件）</span>
          <input
            id="record-keep"
            className="input mono"
            type="number"
            min={1}
            max={100000}
            value={numberInput(settings.recordKeep)}
            onChange={(e) => update({ ...settings, recordKeep: numberValue(e.target.value) })}
          />
        </label>
        <label className="field">
          <span>录制总大小上限（GB）</span>
          <input
            id="record-max-gb"
            className="input mono"
            type="number"
            min={0.5}
            max={10000}
            step="any"
            value={numberInput(settings.recordMaxGb)}
            onChange={(e) => update({ ...settings, recordMaxGb: numberValue(e.target.value) })}
          />
        </label>
      </fieldset>
      {invalid && <div className="notice error" role="alert">{invalid}</div>}
      </>}
      <dl className="kv" style={{ marginTop: 12 }}>
        <dt>引擎</dt>
        <dd className={visibleEngine?.ready ? "ok" : "muted"}>
          {engineLoading ? "正在读取…" : visibleEngine ? `${visibleEngine.backend} · ${visibleEngine.ready ? "就绪" : "未就绪"}${visibleEngine.version ? ` · ${visibleEngine.version}` : ""}` : "--"}
        </dd>
        <dt>说明</dt>
        <dd>{visibleEngine?.message ?? "--"}</dd>
      </dl>
      {engineError && <div className="notice error" role="alert">引擎状态读取失败：{engineError} <button className="btn" onClick={refresh}>重试引擎状态</button></div>}
      <p className="muted hint">
        lyFlow 流程沿各拍照点的示教中线量胶，需要带图像域的 lyFlow 版本；这个流程接入前，图像测量会报“尚未接入”，不用模拟值顶替。帧录制把整帧图像写到数据目录的 records 下，可在图像源页选作回放目录。
      </p>
      {notice && <div className={`notice ${notice.ok ? "ok" : "error"}`} role={notice.ok ? "status" : "alert"}>{notice.text}</div>}
    </div>
  );
}

export default function SettingsPage() {
  const { data: info, error, loading, reload } = useResource<AppInfo>(getAppInfo);
  const savePart = useSavePart();

  return (
    <div className="stack">
      <CycleSettingsPanel savePart={savePart} />
      <MeasurePanel savePart={savePart} />
      <div className="panel">
        <h3 className="panel-title">关于</h3>
        {loading && <p role="status">正在读取应用信息…</p>}
        {error && <div className="notice error" role="alert">{error} <button className="btn" onClick={reload}>重试应用信息</button></div>}
        <dl className="kv">
          <dt>应用</dt>
          <dd>{info?.name ?? "--"}</dd>
          <dt>版本</dt>
          <dd>{info?.version ?? "--"}</dd>
        </dl>
      </div>
    </div>
  );
}
