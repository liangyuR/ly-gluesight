import { CameraSegmentOverview } from "../plan/CameraSegmentOverview";
import { useWorkspace } from "./context";
import { Notice, Panel, WorkspaceBar, WorkspaceEmpty } from "./components";

/** 段列表与配方默认参数（段配方，路由 /recipe/segments）。占位页：段表编辑由 FE2 填充。 */
export default function SegmentsPage() {
  const { data } = useWorkspace();
  if (!data) return <WorkspaceEmpty />;
  const plan = data.plan;
  return <div className="wp-page"><WorkspaceBar />
    <Panel title="段列表（建设中）" detail="沿胶路的直线段与弯道段、各段预期帧数与容差、检测项">
      {plan ? <CameraSegmentOverview cameras={plan.cameras} segments={plan.segments} />
        : <Notice title="当前配方按拍照点模型配置">段列表只用于段配方；拍照点配方继续使用原有步骤。</Notice>}
    </Panel>
  </div>;
}
