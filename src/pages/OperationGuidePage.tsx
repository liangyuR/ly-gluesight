import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { Bug, Camera, Cable, ChevronRight, FlaskConical, GitBranch, LoaderCircle, Play, RefreshCw, ScanEye, Wrench, type LucideIcon } from "lucide-react";
import { Alert, Card, Chip, Heading, IconButton, Stack, StatusBadge } from "qing-steel";
import { cameraApi } from "../features/camera";
import { linkStateLabels, plcApi, subscribe } from "../features/plc";
import { recipeApi } from "../features/cycle";
import { workspaceApi } from "../features/workspace/api";
import { desktopAvailable } from "../lib/desktop";
import { errorText } from "../lib/errors";
import "./operation-guide.css";

type Resource<T> = { data: T | null; error: string; loading: boolean };
function useOverviewResource<T>(request: () => Promise<T>, interval = 0) {
  const [state, setState] = useState<Resource<T>>({ data: null, error: "", loading: true });
  const refreshRef = useRef<(() => void) | null>(null);
  const refresh = useCallback(() => refreshRef.current?.(), []);
  useEffect(() => {
    if (!desktopAvailable()) return;
    let alive = true, pending = false, queued = false;
    const load = async (requested: boolean) => {
      if (!alive) return;
      if (pending) {
        queued ||= requested;
        return;
      }
      pending = true;
      try {
        const data = await request();
        if (alive && !queued) setState({ data, error: "", loading: false });
      } catch (e) {
        if (alive && !queued) setState({ data: null, error: errorText(e), loading: false });
      } finally {
        pending = false;
        if (queued) {
          queued = false;
          void load(false);
        }
      }
    };
    refreshRef.current = () => {
      setState(previous => previous.loading ? previous : { ...previous, loading: true });
      void load(true);
    };
    const timer = interval ? setInterval(() => void load(false), interval) : undefined;
    return () => {
      alive = false;
      refreshRef.current = null;
      clearInterval(timer);
    };
  }, [request, interval]);
  return [state, refresh] as const;
}
function unavailable(state: Resource<unknown>) {
  return !desktopAvailable() ? "需桌面后端" : state.error ? "读取失败" : state.loading ? "读取中…" : "状态未提供";
}
function StatusCard({ title, icon: Icon, status, children, links }: { title: string; icon: LucideIcon; status: ReactNode; children: ReactNode; links: [string, string][] }) {
  return <Card role="region" className="guide-status" aria-label={title}
    title={<Stack gap={10}><span className="guide-icon"><Icon size={21} /></span><Heading level={3}>{title}</Heading></Stack>} actions={status}>
    <div className="guide-status-content">{children}</div>
    <footer>{links.map(([label, path], i) => <Link key={path} className={i === 0 ? "qs-btn tonal sm" : "guide-text-link"} to={path}>{label}</Link>)}</footer>
  </Card>;
}
function ReadError({ state }: { state: Resource<unknown> }) {
  return state.error ? <Alert tone="ng">{state.error}</Alert> : null;
}

const journeys: { title: string; detail: string; icon: LucideIcon; action: string; path: string; links: [string, string][]; production?: boolean }[] = [
  { title: "首次建站", detail: "接入设备，完成采集示教与验证发布", icon: Wrench, action: "配置采集设备", path: "/camera", links: [["相机接入", "/camera"], ["PLC 通讯", "/plc"], ["标定", "/camera/calibration"], ["建立配方", "/recipe"]] },
  { title: "日常生产", detail: "核对开工条件，监看工件检测结果", icon: ScanEye, action: "进入在线检测", path: "/inspect", production: true, links: [["核对开工条件", "/inspect"], ["监看检测", "/inspect"], ["查看历史", "/history"]] },
  { title: "缺陷排查", detail: "查看异常工件原图，选择候选配方复测", icon: Bug, action: "打开历史记录", path: "/history", links: [["定位工件与帧组", "/history"], ["选择候选", "/recipe"], ["调整示教", "/recipe"]] },
  { title: "配置变更", detail: "调整候选配方，完成示教、标定与发布", icon: GitBranch, action: "打开配方库", path: "/recipe", links: [["选择候选", "/recipe"], ["选示教件", "/recipe"], ["发布", "/recipe"], ["核对 PLC 型号", "/recipe"]] },
];

export default function OperationGuidePage() {
  const [cameras, refreshCameras] = useOverviewResource(cameraApi.rigStatus, 5000);
  const [plc, refreshPlc] = useOverviewResource(plcApi.getStatus, 5000);
  const [recipes, refreshRecipes] = useOverviewResource(recipeApi.list);
  const [drafts, refreshDrafts] = useOverviewResource(workspaceApi.list);
  const refreshCatalog = useCallback(() => { refreshRecipes(); refreshDrafts(); }, [refreshRecipes, refreshDrafts]);
  useEffect(() => {
    refreshCameras();
    refreshPlc();
    return subscribe("workspace://changed", refreshCatalog, undefined, refreshCatalog);
  }, [refreshCameras, refreshPlc, refreshCatalog]);
  const refreshAll = () => { refreshCameras(); refreshPlc(); refreshCatalog(); };
  const desktop = desktopAvailable();
  const refreshing = desktop && (cameras.loading || plc.loading || recipes.loading || drafts.loading);
  const ready = cameras.data?.filter(camera => camera.ready).length;
  const cameraLabel = cameras.data ? cameras.data.length ? `${ready}/${cameras.data.length} 设备就绪` : "未配置相机" : unavailable(cameras);
  const pending = drafts.data?.filter(draft => draft.publish.state === "waiting").length;
  const failed = drafts.data?.filter(draft => ["failed", "uncertain"].includes(draft.publish.state)).length;
  const plcMessage = plc.data?.message?.trim();
  const workflows = journeys;
  return <div className="guide-page">
    <div className="guide-toolbar"><div><IconButton variant="text" aria-label="刷新总览状态"
      title={refreshing ? "正在刷新总览状态" : "刷新总览状态"} aria-busy={refreshing} disabled={!desktop} onClick={refreshAll}>
      {refreshing ? <LoaderCircle size={17} /> : <RefreshCw size={17} />}
    </IconButton></div></div>
    <div className="guide-status-grid">
      <StatusCard title="采集设备" icon={Camera} status={<StatusBadge tone={cameras.data?.length && ready === cameras.data.length ? "ok" : "off"}>{cameraLabel}</StatusBadge>} links={[["相机", "/camera"], ["查看标定", "/camera/calibration"]]}>
        <div className="guide-camera-list">{cameras.data?.map(camera => <div className="guide-value" key={camera.id}><span>{camera.name}</span><strong>{camera.ready ? "设备就绪" : camera.released ? "已释放" : "未就绪"}</strong></div>)}</div>
        <ReadError state={cameras} />
      </StatusCard>
      <StatusCard title="PLC 通讯" icon={Cable} status={<StatusBadge tone={plc.data?.state === "connected" ? "ok" : "off"}>{plc.data ? linkStateLabels[plc.data.state] : unavailable(plc)}</StatusBadge>} links={[["PLC 通讯", "/plc"], ["通讯日志", "/plc-logs"]]}>
        <div className="guide-value"><span>通讯协议</span><strong>Siemens S7</strong></div>
        {plc.data && plcMessage && plcMessage !== linkStateLabels[plc.data.state] && <p className="guide-plc-message">{plcMessage}</p>}
        <ReadError state={plc} />
      </StatusCard>
      <StatusCard title="配方版本" icon={FlaskConical} status={<Chip>{recipes.data ? recipes.data.errors.length ? "部分读取失败" : `已发布 ${recipes.data.recipes.length} 个` : unavailable(recipes)}</Chip>} links={[["打开配方库", "/recipe"]]}>
        <div className="guide-value"><span>候选配方</span><strong>{drafts.data ? `${drafts.data.length} 个` : unavailable(drafts)}</strong></div>
        {drafts.data && <div className="guide-value"><span>发布队列</span><strong>{failed || pending ? `${pending} 个待生效 · ${failed} 个需处理` : "无待生效版本"}</strong></div>}
        <ReadError state={recipes} /><ReadError state={drafts} />{recipes.data?.errors.map((error, i) => <Alert tone="ng" key={i}>{error}</Alert>)}
      </StatusCard>
    </div>
    <div className="guide-section-heading"><h2>工作流</h2></div>
    <div className="guide-journeys">{workflows.map(({ title, detail, icon: Icon, action, path, links, production }) => <Card role="region" level={production ? "mid" : "low"} className={`guide-journey${production ? " production" : ""}`} aria-label={title} key={title}>
      <div className="guide-journey-title"><span className="guide-icon"><Icon size={22} /></span><div><h3>{title}</h3><p>{detail}</p></div></div>
      <details className="guide-journey-steps"><summary>查看步骤</summary><nav aria-label={`${title}步骤`}><ol>{links.map(([label, target], i) => <li key={label}><Link to={target}><span>{i + 1}</span>{label}</Link>{i < links.length - 1 && <ChevronRight size={13} aria-hidden="true" />}</li>)}</ol></nav></details>
      <Link className={`qs-btn guide-primary-action${production ? "" : " tonal"}`} to={path}>{production && <Play size={15} />}{action}</Link>
    </Card>)}</div>
  </div>;
}
