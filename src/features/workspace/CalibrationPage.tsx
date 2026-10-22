import { CameraSegmentOverview } from "../plan/CameraSegmentOverview";
import { useWorkspace } from "./context";
import { Notice, Panel, WorkspaceBar, WorkspaceEmpty } from "./components";

/** 多轮标定（段配方，路由 /recipe/calibration）。占位页：轮次列表、N_exp 统计与容差确认由 FE3 填充。 */
export default function CalibrationPage() {
  const { data } = useWorkspace();
  if (!data) return <WorkspaceEmpty />;
  const plan = data.plan;
  return <div className="wp-page"><WorkspaceBar />
    <Panel title="多轮标定（建设中）" detail="同一工件程序连续采集多轮，取帧组数均值作预期帧数，再确认容差">
      {plan ? <CameraSegmentOverview cameras={plan.cameras} segments={plan.segments} />
        : <Notice title="当前配方按拍照点模型配置">多轮标定只用于段配方；拍照点配方继续使用原有步骤。</Notice>}
    </Panel>
  </div>;
}
