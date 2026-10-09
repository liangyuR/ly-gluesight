import { Link, Outlet, useLocation } from "react-router-dom";
import { LayoutGrid } from "lucide-react";
import ErrorBoundary from "./ErrorBoundary";
import Sidebar from "./Sidebar";
import { navItems } from "./nav";
import { desktopAvailable } from "../lib/desktop";

export default function AppLayout() {
  const { pathname } = useLocation();
  const current = navItems.filter((item) => pathname === item.path || pathname.startsWith(item.path + "/")).sort((a,b) => b.path.length - a.path.length)[0];

  return (
    <div className="app-shell">
      <Sidebar />
      <main className="main">
        <header className="main-header">
          <h1>{current?.label ?? ""}</h1>
          <Link to="/workflow/guide" className="btn workflow-entry"><LayoutGrid size={16} />操作流程预览</Link>
        </header>
        <section className="main-body">
          <ErrorBoundary key={pathname}>
            {!desktopAvailable() && <div className="wp-native-mode">浏览器查看模式：设备、配方保存与生产操作需要桌面后端。</div>}
            <Outlet />
          </ErrorBoundary>
        </section>
      </main>
    </div>
  );
}
