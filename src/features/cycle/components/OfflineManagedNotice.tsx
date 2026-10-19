import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { Alert } from "qing-steel";

export default function OfflineManagedNotice({ children }: { children?: ReactNode }) {
  return (
    <Alert tone="warn" title="离线模式由站点模式管理">
      {children} 退出离线后恢复原配置。<Link to="/offline">打开离线模式</Link>
    </Alert>
  );
}
