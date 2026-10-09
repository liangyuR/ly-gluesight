import { Link } from "react-router-dom";
import { ChevronRight } from "lucide-react";
import { useRigStatus } from "../features/camera";
import { usePlcStatus } from "../features/plc";
import { useWorkspace } from "../features/workspace/context";
import { Badge, Notice, Panel } from "../features/workspace/components";

const journeys=[
  {title:"首次建站",detail:"连接采集设备和 PLC，完成工位标定、配方示教与发布。",links:[["设备与采集","/camera"],["PLC 通讯","/plc"],["工位标定","/camera/calibration"],["配方库","/recipe"]]},
  {title:"日常生产",detail:"确认当前生产版本与设备就绪，检查工件总览、原图及检测结论。",links:[["在线检测","/inspect"],["历史记录","/history"]]},
  {title:"缺陷排查",detail:"从整件结论定位到帧和原图，区分缺陷与测量异常，再进行候选复测。",links:[["历史记录","/history"],["历史复测","/history/retest"],["单帧示教","/recipe/teach"]]},
  {title:"配置变更",detail:"修改候选配置、重新试测与验证，发布后在工件边界生效。",links:[["拍照规划","/recipe/geometry"],["单帧示教","/recipe/teach"],["工件总览","/recipe/overview"],["验证与发布","/recipe/validation"]]},
];
export default function OperationGuidePage(){
  const {statuses}=useRigStatus();
  const plc=usePlcStatus();
  const {list,drafts}=useWorkspace();
  return <div className="wp-page"><Notice title="从建站到生产，配置按步骤完成">候选配置独立保存。图像试测和规则验证通过后，发布生产配方；在线工件保持开始检测时的版本。</Notice><div className="wp-three"><Panel title="采集设备" actions={<Badge tone={statuses.length&&statuses.every(s=>s.ready)?"ok":"warn"}>{statuses.filter(s=>s.ready).length}/{statuses.length} 就绪</Badge>}><p className="muted">检查连接、采集方式、曝光和相机实际接受的参数。</p><Link className="btn" to="/camera">配置采集设备</Link></Panel><Panel title="PLC 通讯" actions={<Badge tone={plc?.state==="connected"?"ok":"warn"}>{plc?.state==="connected"?"已连接":"未连接"}</Badge>}><p className="muted">通讯连接后，还需检查工件开始、布防、结果和确认点位。</p><Link className="btn" to="/plc">检查通讯与点位</Link></Panel><Panel title="配方版本" actions={<Badge tone="neutral">{list.length} 个生产配方</Badge>}><p className="muted">{drafts.length} 个候选工作区。已开始的工件继续使用原生产快照。</p><Link className="btn" to="/recipe">打开配方库</Link></Panel></div><div className="wp-library-grid">{journeys.map(j=><Panel key={j.title} title={j.title} className="wp-guide-card"><p>{j.detail}</p><div className="wp-guide-links">{j.links.map(([label,path],i)=><span key={path} className="wp-actions"><Link to={path}>{label}</Link>{i<j.links.length-1&&<ChevronRight size={12}/>}</span>)}</div><Link className="btn" to={j.links[0][1]} style={{marginTop:12}}>开始{j.title}<ChevronRight size={14}/></Link></Panel>)}</div></div>;
}
