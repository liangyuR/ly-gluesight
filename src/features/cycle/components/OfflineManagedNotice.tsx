import type { ReactNode } from "react";
import { Link } from "react-router-dom";

export default function OfflineManagedNotice({ children }: { children?: ReactNode }) {
  return (
    <div className="site-managed" role="note">
      <strong>离线模式由站点模式管理</strong>
      <span>{children} 退出离线后恢复原配置。<Link to="/offline">打开离线控制台</Link></span>
    </div>
  );
}
