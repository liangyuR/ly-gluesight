import { Outlet, useLocation } from "react-router-dom";
import ErrorBoundary from "./ErrorBoundary";
import Sidebar from "./Sidebar";
import { desktopAvailable } from "../lib/desktop";

export default function AppLayout() {
  const { pathname } = useLocation();

  return (
    <div className="app-shell">
      <Sidebar />
      <main className="main">
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
