import { Outlet, useLocation } from "react-router-dom";
import ErrorBoundary from "./ErrorBoundary";
import Sidebar from "./Sidebar";
import { matchRoute, pageRoutes } from "./nav";
import { desktopAvailable } from "../lib/desktop";
import { SiteModeBadge } from "../features/cycle";

export default function AppLayout() {
  const { pathname } = useLocation();
  const current = matchRoute(pageRoutes, pathname);

  return (
    <div className="app-shell">
      <Sidebar />
      <main className="main">
        <header className="main-header">
          <h1>{current?.label ?? ""}</h1>
          <SiteModeBadge />
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
