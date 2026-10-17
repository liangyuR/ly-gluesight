import { useEffect, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { Bug, Camera, Cable, ChevronRight, FlaskConical, GitBranch, Play, RefreshCw, ScanEye, Wrench, type LucideIcon } from "lucide-react";
import { cameraApi } from "../features/camera";
import { linkStateLabels, plcApi, subscribe } from "../features/plc";
import { recipeApi, siteApi, siteModeText } from "../features/cycle";
import { workspaceApi } from "../features/workspace/api";
import { Badge } from "../features/workspace/components";
import { desktopAvailable } from "../lib/desktop";
import { errorText } from "../lib/errors";
import "./operation-guide.css";

type Resource<T> = { data: T | null; error: string; loading: boolean };
function useOverviewResource<T>(request: () => Promise<T>, refresh: number, interval = 0) {
  const [state, setState] = useState<Resource<T>>({ data: null, error: "", loading: true });
  useEffect(() => {
    if (!desktopAvailable()) return;
    let alive = true, pending = false;
    const load = async () => {
      if (pending) return;
      pending = true;
      try {
        const data = await request();
        if (alive) setState({ data, error: "", loading: false });
      } catch (e) {
        if (alive) setState({ data: null, error: errorText(e), loading: false });
      } finally { pending = false; }
    };
    setState({ data: null, error: "", loading: true });
    void load();
    const timer = interval ? setInterval(() => void load(), interval) : undefined;
    return () => { alive = false; clearInterval(timer); };
  }, [request, refresh, interval]);
  return state;
}
function unavailable(state: Resource<unknown>) {
  return !desktopAvailable() ? "需桌面后端" : state.error ? "读取失败" : state.loading ? "读取中…" : "状态未提供";
}
function StatusCard({ title, icon: Icon, status, children, links }: { title: string; icon: LucideIcon; status: ReactNode; children: ReactNode; links: [string, string][] }) {
  return <section className="guide-status" aria-label={title}>
    <header><span className="guide-icon"><Icon size={21} /></span><h2>{title}</h2>{status}</header>
    <div className="guide-status-content">{children}</div>
    <footer>{links.map(([label, path], i) => <Link key={path} className={i === 0 ? "btn small" : "guide-text-link"} to={path}>{label}</Link>)}</footer>
  </section>;
}
function ReadError({ state }: { state: Resource<unknown> }) {
  return state.error ? <p className="guide-error" role="alert">{state.error}</p> : null;
}

const journeys: { title: string; detail: string; icon: LucideIcon; action: string; path: string; links: [string, string][]; production?: boolean }[] = [
  { title: "首次建站", detail: "接入设备，完成采集示教与验证发布", icon: Wrench, action: "配置采集设备", path: "/camera", links: [["相机接入", "/camera"], ["PLC 通讯", "/plc"], ["飞拍标定", "/camera/calibration"], ["建立配方", "/recipe"]] },
  { title: "日常生产", detail: "开工条件与当前工件状态以在线检测为准", icon: ScanEye, action: "进入在线检测", path: "/inspect", production: true, links: [["核对开工条件", "/inspect"], ["监看检测", "/inspect"], ["查看历史", "/history"]] },
  { title: "缺陷排查", detail: "先查历史帧与原图，再选择候选复测", icon: Bug, action: "打开历史记录", path: "/history", links: [["定位工件与帧", "/history"], ["选择候选", "/recipe"], ["调整示教", "/recipe/teach"]] },
  { title: "配置变更", detail: "候选验证后发布；在途工件保持原快照", icon: GitBranch, action: "打开配方库", path: "/recipe", links: [["选择候选", "/recipe"], ["采集 / 示教", "/recipe/capture"], ["验证与发布", "/recipe/validation"], ["核对 PLC 计划", "/plc?panel=s7-plan"]] },
];

export default function OperationGuidePage() {
  const [refresh, setRefresh] = useState(0);
  useEffect(() => subscribe("workspace://changed", () => setRefresh(value => value + 1)), []);
  const cameras = useOverviewResource(cameraApi.rigStatus, refresh, 5000);
  const plc = useOverviewResource(plcApi.getStatus, refresh, 5000);
  const recipes = useOverviewResource(recipeApi.list, refresh);
  const drafts = useOverviewResource(workspaceApi.list, refresh);
  const site = useOverviewResource(siteApi.status, refresh, 5000);
  const ready = cameras.data?.filter(camera => camera.ready).length;
  const cameraLabel = cameras.data ? cameras.data.length ? `${ready}/${cameras.data.length} 设备就绪` : "未配置相机" : unavailable(cameras);
  const pending = drafts.data?.filter(draft => draft.publish.state === "waiting").length;
  const failed = drafts.data?.filter(draft => ["failed", "uncertain"].includes(draft.publish.state)).length;
  return <div className="guide-page">
    <div className="guide-toolbar"><div><Link className="guide-text-link" to="/offline">{site.data ? siteModeText(site.data) : unavailable(site)} · 离线模式</Link><button className="icon-btn" aria-label="刷新总览状态" disabled={!desktopAvailable()} onClick={() => setRefresh(value => value + 1)}><RefreshCw size={17} /></button></div></div>
    <ReadError state={site} />
    <div className="guide-status-grid">
      <StatusCard title="采集设备" icon={Camera} status={<Badge tone={cameras.data?.length && ready === cameras.data.length ? "ok" : "neutral"}>{cameraLabel}</Badge>} links={[["相机与采集", "/camera"], ["查看标定", "/camera/calibration"]]}>
        <div className="guide-camera-list">{cameras.data?.map(camera => <div className="guide-value" key={camera.id}><span>{camera.name}</span><strong>{camera.ready ? "设备就绪" : camera.released ? "已释放" : "未就绪"}</strong><small>{camera.message}</small></div>)}</div>
        <ReadError state={cameras} />
      </StatusCard>
      <StatusCard title="PLC 通讯" icon={Cable} status={<Badge tone={plc.data?.state === "connected" ? "ok" : "neutral"}>{plc.data ? linkStateLabels[plc.data.state] : unavailable(plc)}</Badge>} links={[["PLC 通讯", "/plc"], ["通讯日志", "/plc-logs"]]}>
        <div className="guide-value"><span>通讯协议</span><strong>Siemens S7</strong></div>
        <p className="guide-plc-message">{plc.data?.message || (plc.data ? "检查工件开始、布防、结果与确认点位" : unavailable(plc))}</p>
        <ReadError state={plc} />
      </StatusCard>
      <StatusCard title="配方版本" icon={FlaskConical} status={<Badge tone="neutral">{recipes.data ? recipes.data.errors.length ? "部分读取失败" : `${recipes.data.recipes.length} 个已发布` : unavailable(recipes)}</Badge>} links={[["打开配方库", "/recipe"]]}>
        <div className="guide-value"><span>已加载候选</span><strong>{drafts.data ? `${drafts.data.length} 个` : unavailable(drafts)}</strong></div>
        {drafts.data && <div className="guide-value"><span>已加载发布</span><strong>{failed || pending ? `${pending} 个待生效 · ${failed} 个需处理` : "无待生效版本"}</strong></div>}
        <ReadError state={recipes} /><ReadError state={drafts} />{recipes.data?.errors.map((error, i) => <p className="guide-error" role="alert" key={i}>{error}</p>)}
      </StatusCard>
    </div>
    <div className="guide-section-heading"><h2>工作流</h2></div>
    <div className="guide-journeys">{journeys.map(({ title, detail, icon: Icon, action, path, links, production }) => <section className={`guide-journey${production ? " production" : ""}`} aria-label={title} key={title}>
      <div className="guide-journey-title"><span className="guide-icon"><Icon size={22} /></span><div><h3>{title}</h3><p>{detail}</p></div></div>
      <nav aria-label={`${title}步骤`}><ol>{links.map(([label, target], i) => <li key={label}><Link to={target}><span>{i + 1}</span>{label}</Link>{i < links.length - 1 && <ChevronRight size={13} aria-hidden="true" />}</li>)}</ol></nav>
      <Link className={`btn guide-primary-action${production ? " primary" : ""}`} to={path}>{production && <Play size={15} />}{action}</Link>
    </section>)}</div>
  </div>;
}
