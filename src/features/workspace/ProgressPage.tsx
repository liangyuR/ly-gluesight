import { Link } from "react-router-dom";
import { useWorkspace } from "./context";
import { Badge, KV, Panel, Steps, WorkspaceBar, WorkspaceEmpty } from "./components";
import { selectedViews, teachingCounts } from "./teach";

export default function ProgressPage() {
  const { data, doc } = useWorkspace();
  if (!data || !doc) return <WorkspaceEmpty/>;
  const w = data.workspace, count = teachingCounts(doc.shots,w.frames);
  const calibrated = count.total > 0 && doc.shots.every((shot,k) => shot.skip || (selectedViews(shot).length > 0 && selectedViews(shot).every(view => w.frames[k]?.viewStates?.find(v => v.view === view)?.calibrationCheck?.passed)));
  const teaching = count.total > 0 && count.completed === count.total && doc.shots.every(s => s.skip || selectedViews(s).length > 0);
  const next = !w.captureId || !w.frames.length ? "/recipe/capture" : !teaching || !calibrated ? "/recipe/teach?frame=" + (w.lastPosition?.k ?? 0) : "/recipe/validation";
  return <div className="wp-page"><WorkspaceBar/><Steps/><Panel title="配方创建进度" detail="草稿、示教完成、验证通过与生产生效分别记录" actions={<Link className="btn primary" to={next}>继续配置</Link>}>
    <div className="wp-checklist">{[
      ["设备与整圈采集",w.captureId ? "已采用完整采集轮次" : "等待设备就绪并采集", "/recipe/capture"],
      ["采集结构",`${w.frames.length} 个拍照点`,"/recipe/capture"],
      ["毫米标定",calibrated && w.frames.length ? "所选图验证满足 ±0.1 mm" : "待验证所选图的标定精度","/recipe/teach"],
      ["逐幅示教",`${count.completed} / ${count.total} 幅完成`,"/recipe/teach?frame=" + (w.lastPosition?.k ?? 0)],
      ["正常样本验证",w.validation?.passed && w.validation.revision === w.revision ? "当前修订验证通过" : "待独立正常整圈样本验证","/recipe/validation"],
      ["发布生效",w.publishError ? "发布失败，可重试" : w.pending ? "等待工件边界" : data.productionVersion ? `生产 v${data.productionVersion}` : "未发布","/recipe/validation"],
    ].map(([name,status,to]) => <div className="wp-check-row" key={name}><div><strong>{name}</strong><p>{status}</p></div><Link className="btn small" to={to}>查看</Link></div>)}</div>
    <KV label="上次编辑">{w.lastPosition ? `拍照点 ${w.lastPosition.k+1} · 图 ${w.lastPosition.view}` : "尚未开始示教"}</KV><Badge tone="neutral">自动保存仅保留编辑进度</Badge>
  </Panel><div className="wp-actions"><Link className="btn" to="/recipe/geometry">配方默认参数</Link><Link className="btn" to="/recipe/overview">工件显示布置</Link></div></div>;
}
