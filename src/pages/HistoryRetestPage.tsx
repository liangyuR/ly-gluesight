import { Link } from "react-router-dom";
import { Notice, Panel } from "../features/workspace/components";

export default function HistoryRetestPage(){
  return <div className="wp-page"><Notice title="先选择需要排查的工件">历史复测保留原始结果。原包重现使用本件冻结的发布资源；候选原图复测使用当前示教；规则重判使用布局兼容的已存测量数据。</Notice><Panel title="选择历史工件" detail="在历史记录中打开一件工件，再选择拍照点、视角和复测方式"><p className="muted">没有原图时仍可重判完整测量数据；缺帧或未量成的记录需要重新采集或回放。</p><Link className="btn primary" to="/history">打开历史记录</Link></Panel></div>;
}
