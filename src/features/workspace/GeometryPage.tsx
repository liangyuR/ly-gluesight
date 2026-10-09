import { Link } from "react-router-dom";
import { RecipeEditor } from "../recipe";
import { FeasibilityCalc } from "../camera";
import { shotFov } from "../cycle/vis";
import { useWorkspace } from "./context";
import { Badge, Panel, Steps, WorkspaceBar, WorkspaceEmpty } from "./components";

export default function GeometryPage() {
  const {data,doc,cameras,setDoc,saveDoc,preview,busy} = useWorkspace();
  if (!data || !doc) return <WorkspaceEmpty />;
  // 测点连同搜索余量落在某个拍照点的视野内才算覆盖；视野按拍照点自己的，缺省用配方视野
  const cover = preview ? preview.points.x.reduce((count,x,j) => count + Number(preview.shots.some((shot,k) => {
    const [cx,cy] = shot.center, [fw,fh] = shotFov(preview,k);
    const margin = data.workspace.frames[k]?.params.searchMm ?? 4;
    return Math.abs(x-cx) + margin <= fw/2 && Math.abs(preview.points.y[j]-cy) + margin <= fh/2;
  })),0) / Math.max(1,preview.points.x.length) * 100 : data.coverage;
  // 曝光参考：当前草稿第一个拍照点的相机
  const camera = doc.shots[0]?.camera;
  return <div className="wp-page"><WorkspaceBar /><Steps /><div className="wp-actions"><Badge tone={cover >= 99.995 ? "ok" : "warn"}>{"搜索窗口覆盖 " + cover.toFixed(2) + "%"}</Badge><span className="muted">布置坐标用于真实拍照规划；总览显示位置在下一步骤单独调整。</span><span className="spacer" /><Link className="btn" to="/recipe/teach">进入单帧示教</Link></div>
    <Panel title="候选胶路与检测规则" detail="保存候选配置不会改变在线使用的生产配方">
      <fieldset disabled={busy} style={{border:0,padding:0,margin:0,minWidth:0}}>
      <RecipeEditor key={doc.id + ":" + data.workspace.revision} initial={doc} originalId={data.workspace.baseHash ? doc.id : null} cameras={cameras} onSaved={() => {}} onDraftChange={setDoc} saveCandidate={async candidate => !!(await saveDoc(candidate))} />
      </fieldset>
    </Panel>
    <FeasibilityCalc exposure={cameras.find(c => c.id === camera)?.exposureUs ?? 60} fps={undefined} />
  </div>;
}
