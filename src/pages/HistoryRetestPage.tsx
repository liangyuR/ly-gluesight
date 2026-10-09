import { Link } from "react-router-dom";
import { Notice, Panel } from "../features/workspace/components";

export default function HistoryRetestPage(){
  return <div className="wp-page"><Notice title="先选择需要排查的工件">历史复测保留原始结果。规则重判使用已存储的测量数据；原图复测重新运行当前候选的图像测量。</Notice><Panel title="选择历史工件" detail="在历史记录中打开一件工件，再选择帧和复测方式"><p className="muted">没有原图时仍可重判完整测量数据；缺帧或未量成的记录需要重新采集或回放。</p><Link className="btn primary" to="/history">打开历史记录</Link></Panel></div>;
}
