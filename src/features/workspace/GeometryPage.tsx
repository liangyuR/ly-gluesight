import { Link } from "react-router-dom";
import { RecipeEditor } from "../recipe";
import { FeasibilityCalc } from "../camera";
import { useWorkspace } from "./context";
import { Badge, Panel, Steps, WorkspaceBar, WorkspaceEmpty } from "./components";

export default function GeometryPage() {
  const {data,doc,cameras,setDoc,saveDoc,preview,busy} = useWorkspace();
  if (!data || !doc) return <WorkspaceEmpty />;
  const cover = preview ? preview.points.x.reduce((count,x,j) => count + Number(preview.shots.some(([cx,cy],k) => {
    const margin = data.workspace.frames[k]?.params.searchMm ?? 4;
    return Math.abs(x-cx) + margin <= preview.fov[0]/2 && Math.abs(preview.points.y[j]-cy) + margin <= preview.fov[1]/2;
  })),0) / Math.max(1,preview.points.x.length) * 100 : data.coverage;
  return <div className="wp-page"><WorkspaceBar /><Steps /><div className="wp-actions"><Badge tone={doc.mode === "follow" || cover >= 99.995 ? "ok" : "warn"}>{doc.mode === "follow" ? "随动胶路" : "搜索窗口覆盖 " + cover.toFixed(2) + "%"}</Badge><span className="muted">布置坐标用于真实拍照规划；总览显示位置在下一步骤单独调整。</span><span className="spacer" /><Link className="btn" to={doc.mode === "follow" ? "/camera/follow" : "/recipe/teach"}>{doc.mode === "follow" ? "相机标定" : "进入单帧示教"}</Link></div>
    <Panel title="候选胶路与检测规则" detail="保存候选配置不会改变在线使用的生产配方">
      <fieldset disabled={busy} style={{border:0,padding:0,margin:0,minWidth:0}}>
      <RecipeEditor key={doc.id + ":" + data.workspace.revision} initial={doc} originalId={data.workspace.baseHash ? doc.id : null} cameras={cameras} onSaved={() => {}} onDraftChange={setDoc} saveCandidate={async candidate => !!(await saveDoc(candidate))} />
      </fieldset>
    </Panel>
    {doc.mode === "flyShot" && <FeasibilityCalc exposure={cameras.find(c => c.id === doc.camera)?.exposureUs ?? 60} fps={undefined} />}
  </div>;
}
