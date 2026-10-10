import { Link } from "react-router-dom";
import { RecipeEditor } from "../recipe";
import { FeasibilityCalc } from "../camera";
import { shotTaught, taughtPercent } from "../cycle/vis";
import { useWorkspace } from "./context";
import { Badge, Panel, Steps, WorkspaceBar, WorkspaceEmpty } from "./components";

/** 拍照点规划：拍照点表（相机、胶条、不检、示教状态）、默认检测参数与限值；中线在单帧示教里点出。 */
export default function GeometryPage() {
  const {data,doc,cameras,setDoc,saveDoc,preview,busy} = useWorkspace();
  if (!data || !doc) return <WorkspaceEmpty />;
  // 要检的拍照点里已示教中线的比例：按正在编辑的预览算，预览暂缺时用工作台给的
  const taught = preview ? taughtPercent(preview.shots) : data.coverage;
  const untaught = preview ? preview.shots.filter(s => !s.skip && !shotTaught(s)).map(s => s.id) : [];
  // 曝光参考：当前草稿第一个拍照点的相机
  const camera = doc.shots[0]?.camera;
  return <div className="wp-page"><WorkspaceBar /><Steps /><div className="wp-actions"><Badge tone={taught >= 99.995 ? "ok" : "warn"}>{"胶路示教 " + taught.toFixed(0) + "%"}</Badge><span className="muted">{untaught.length ? "未示教：" + untaught.join("、") + "，可以保存，但不能开工。" : "每个拍照点在自己的图像里沿示教中线量胶；中线在单帧示教里点出。"}</span><span className="spacer" /><Link className="btn" to="/recipe/teach">进入单帧示教</Link></div>
    <Panel title="候选拍照点与检测规则" detail="保存候选配置不会改变在线使用的生产配方">
      <fieldset disabled={busy} style={{border:0,padding:0,margin:0,minWidth:0}}>
      <RecipeEditor key={doc.id + ":" + data.workspace.revision} initial={doc} originalId={data.workspace.baseRevision ? doc.id : null} cameras={cameras} onSaved={() => {}} onDraftChange={setDoc} saveCandidate={async candidate => !!(await saveDoc(candidate))} />
      </fieldset>
    </Panel>
    <FeasibilityCalc exposure={cameras.find(c => c.id === camera)?.exposureUs ?? 60} fps={undefined} />
  </div>;
}
