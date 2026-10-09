import { useCallback, useEffect, useReducer, useState } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";
import { ArrowRight, ArrowUpRight, Check, ChevronRight, LayoutGrid, RotateCcw, ScanLine } from "lucide-react";
import { WorkflowContext, useWorkflow } from "../features/workflow/context";
import { initialState, reducer, restorePreview, sceneState, type View, type WorkflowState } from "../features/workflow/model";
import { journeys, routeTo, scenes, screens } from "../features/workflow/catalog";
import { Badge, Dialog, Notice, OverviewMap, Panel } from "../features/workflow/components";
import { CalibrationView, DeviceView, FollowView, PlcView, SettingsView } from "../features/workflow/SetupViews";
import { GeometryView, OverviewView, RecipesView, TeachView, ValidationView } from "../features/workflow/RecipeViews";
import { HistoryView, LiveView, RecordView } from "../features/workflow/ProductionViews";
import "../features/workflow/workflow.css";

const storageKey = "tujiao-workflow-preview-v3";
function loadPreview(): WorkflowState {
  try {
    const stored = sessionStorage.getItem(storageKey);
    if (stored) {
      return restorePreview(JSON.parse(stored));
    }
  } catch { /* Browser storage is optional for this preview. */ }
  return initialState();
}
function GuideView() {
  const { state: s, dispatch, go } = useWorkflow();
  const begin = (index: number) => {
    const state = initialState();
    if (index === 0) {
      state.device.connected = false; state.device.applied = false;
      state.plc.connected = false; state.plc.ready = false;
      state.calibration = { captured: false, result: "idle", saved: false, sample: "good", version: 7 };
      state.frames = state.frames.map(f => ({ ...f, imageId: null, trial: null, saved: false }));
      state.selectedFrame = 1;
    }
    if (index === 4) state.recipe.mode = "follow";
    dispatch({ type: "load", state });
    go(journeys[index].views[0]);
  };
  return <>
    <div className="wf-guide-hero"><div><Badge>本地交互原型</Badge><h2>看清整件，<br />把每一帧教准确。</h2><p>从设备准备到单帧示教、验证发布与历史复盘。<br />纯黑工作区，蓝色标记当前选择和主要操作。</p><div className="wf-actions"><button className="btn primary" onClick={() => go("teach")}><ScanLine size={16} />进入单帧示教</button><button className="btn" onClick={() => go("live")}>查看在线检测<ArrowRight size={16} /></button></div></div><div className="wf-guide-map"><OverviewMap /><div className="wf-guide-map-meta"><span>工件 A · 飞拍 · 6 帧</span><Badge>候选 v{s.recipe.candidate}</Badge></div></div></div>
    <div className="wf-guide-stats"><div><strong>13</strong><span>操作页面</span></div><div><strong>32</strong><span>关键情景</span></div><div><strong>5</strong><span>完整操作路径</span></div><div><strong>0</strong><span>新增付费依赖</span></div></div>
    <Panel title="按任务开始" detail="路径中的步骤均可点击；开始路径会载入对应示例状态"><div className="wf-journeys">{journeys.map((j, i) => <article key={j.title}><div className="wf-journey-title"><span>{String(i + 1).padStart(2, "0")}</span><div><h3>{j.title}</h3><p className="wf-caption">{j.detail}</p></div><button className="btn" onClick={() => begin(i)}>开始此路径<ArrowRight size={14} /></button></div><div className="wf-journey-steps">{j.views.map((view, n) => <span key={n}><button onClick={() => go(view)}>{screens.find(s => s.id === view)?.label}</button>{n < j.views.length - 1 && <ChevronRight size={12} />}</span>)}</div></article>)}</div></Panel>
    <div className="wf-reference-grid"><Panel title="总览 → 帧 → 原图" detail="参考手册第 39、73–78 页"><p className="wf-caption">总览保留工件轮廓和帧位置。点击帧框，原图同步切换。显示布置与物理测量坐标独立。</p></Panel><Panel title="冻结 → 试匹配 → 保存" detail="参考手册第 79、104 页"><p className="wf-caption">每帧绑定冻结图像与参数。试匹配通过后保存；采集或参数变更会使相关验证失效。</p></Panel><Panel title="历史样本 → 验证 → 发布" detail="参考手册第 111–115、120 页"><p className="wf-caption">历史样本可进入示教并保留原图备份。代表性样本验证通过后，在工件边界更新生产版本。</p></Panel></div>
    <Notice title="当前为操作流程预览">图像、设备状态和测量结果使用示例数据。可体验操作与限制，真实检测继续通过现有软件和算法接口完成。</Notice>
  </>;
}
function ScenePicker({ onClose }: { onClose: () => void }) {
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const [group, setGroup] = useState("全部");
  const filtered = scenes.filter(s => (group === "全部" || s.group === group) && s.label.toLowerCase().includes(query.trim().toLowerCase()));
  return <Dialog title="全部关键情景 · 32" onClose={onClose}><p className="wf-caption">选择情景会重置当前预览数据，便于检查成功、异常和未完成状态。</p><div className="wf-scene-filter"><input className="input" placeholder="查找情景" aria-label="查找情景" value={query} onChange={e => setQuery(e.target.value)} /><select className="input" aria-label="情景分类" value={group} onChange={e => setGroup(e.target.value)}>{["全部", "建站", "配方", "示教", "验证", "生产", "历史", "异常"].map(g => <option key={g}>{g}</option>)}</select><button className="btn" disabled={!query && group === "全部"} onClick={() => { setQuery(""); setGroup("全部"); }}>清空筛选</button></div>{!filtered.length && <Notice title="没有匹配的情景">调整查询文字或情景分类。</Notice>}<div className="wf-scene-grid">{filtered.map((s, i) => <button key={s.id} onClick={() => { navigate(routeTo(s.view) + "?scene=" + s.id); onClose(); }}><span>{s.group} · {String(i + 1).padStart(2, "0")}</span><strong>{s.label}</strong><small>{screens.find(x => x.id === s.view)?.label}<ArrowUpRight size={13} /></small></button>)}</div></Dialog>;
}

export default function WorkflowPreviewPage() {
  const [state, dispatch] = useReducer(reducer, undefined, loadPreview);
  const [toast, setToast] = useState("");
  const [picker, setPicker] = useState(false);
  const navigate = useNavigate();
  const location = useLocation();
  const params = useParams();
  const view = screens.some(s => s.id === params.view) ? params.view as View : "guide";
  const current = screens.find(s => s.id === view)!;
  const sceneId = new URLSearchParams(location.search).get("scene");
  const notify = useCallback((message: string) => setToast(message), []);
  const go = useCallback((to: View) => navigate(routeTo(to)), [navigate]);

  useEffect(() => {
    if (sceneId && scenes.some(s => s.id === sceneId)) dispatch({ type: "load", state: sceneState(sceneId) });
  }, [sceneId, location.key]);
  useEffect(() => { try { sessionStorage.setItem(storageKey, JSON.stringify(state)); } catch { /* Keep the preview usable without storage. */ } }, [state]);
  useEffect(() => { if (!toast) return; const timer = setTimeout(() => setToast(""), 4200); return () => clearTimeout(timer); }, [toast]);
  useEffect(() => {
    if (!state.live.auto || state.live.phase < 1 || state.live.phase > 4) return;
    if (state.live.phase === 4 && (!state.live.continuous || !state.live.accepting)) return;
    const timer = setTimeout(() => dispatch({ type: state.live.phase === 4 ? "live-next" : "live-step" }), 1600);
    return () => clearTimeout(timer);
  }, [state.live.auto, state.live.phase, state.live.continuous, state.live.accepting]);
  useEffect(() => { document.querySelector(".wf-body")?.scrollTo(0, 0); }, [view, sceneId]);

  const render = () => {
    switch (view) {
      case "device": return <DeviceView />;
      case "plc": return <PlcView />;
      case "calibration": return <CalibrationView />;
      case "follow": return <FollowView />;
      case "recipes": return <RecipesView />;
      case "geometry": return <GeometryView />;
      case "teach": return <TeachView />;
      case "overview": return <OverviewView />;
      case "validation": return <ValidationView openConfirm={sceneId === "publish-confirm"} />;
      case "live": return <LiveView />;
      case "history": return <HistoryView />;
      case "record": return <RecordView />;
      case "settings": return <SettingsView />;
      default: return <GuideView />;
    }
  };
  return <WorkflowContext.Provider value={{ state, dispatch, go, notify }}>
    <div className="wf-shell" data-scene={state.scene} data-view={view}>
      <aside className="wf-sidebar"><div className="sidebar-brand"><img className="brand-logo" src="/app-icon.png" alt="" /><div className="brand-text"><strong>GlueSight</strong><span>胶路智检 · 操作流程预览</span></div></div><nav className="wf-navigation" aria-label="操作流程导航">{[["guide", ""], ["production", "生产与复盘"], ["recipe", "配方工作台"], ["setup", "设备与运行"]].map(([group, label]) => <div key={group}>{label && <div className="wf-nav-label">{label}</div>}{screens.filter(s => s.group === group).map(item => <Link key={item.id} to={routeTo(item.id)} className={"wf-nav-item " + (view === item.id ? "active" : "")} aria-current={view === item.id ? "page" : undefined}><item.icon size={17} /><span>{item.label}</span></Link>)}</div>)}</nav><footer className="wf-sidebar-footer"><div><span className="wf-log-dot" />示例数据 · 本地运行</div><Link to="/inspect">返回现有软件<ArrowUpRight size={15} /></Link></footer></aside>
      <main className="wf-main"><header className="wf-header"><div><h1>{current.label}</h1><p>{current.description}</p></div><div className="wf-row"><button className="btn" onClick={() => setPicker(true)}><LayoutGrid size={16} />全部情景</button><button className="icon-btn" aria-label="重置预览数据" title="重置预览数据" onClick={() => { dispatch({ type: "load", state: initialState() }); navigate(routeTo(view), { replace: true }); notify("预览数据已恢复到初始状态。"); }}><RotateCcw size={17} /></button></div></header><div className="wf-preview-banner"><span><span className="wf-preview-dot" />操作流程预览 · 图像、设备与测量均为示例</span><span>示例状态，可随时重置</span></div><div className="wf-body" key={view + ":" + sceneId + ":" + location.key}>{render()}</div></main>
      {toast && <div className="wf-toast" role="status"><Check size={17} /><span>{toast}</span></div>}
      {picker && <ScenePicker onClose={() => setPicker(false)} />}
    </div>
  </WorkflowContext.Provider>;
}
