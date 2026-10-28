import { useRef } from "react";
import { Outlet, useLocation } from "react-router-dom";
import { Alert } from "qing-steel";
import ErrorBoundary from "./ErrorBoundary";
import Sidebar from "./Sidebar";
import WindowFrame from "./WindowFrame";
import { desktopAvailable } from "../lib/desktop";

export default function AppLayout() {
  const { pathname } = useLocation();
  const main = useRef<HTMLElement>(null);

  return (
    <WindowFrame>
      <div className="app-shell">
        <Sidebar main={main} />
        <main ref={main} className="main">
          <section className="main-body">
            <ErrorBoundary key={pathname}>
              {!desktopAvailable() && <div className="native-mode"><Alert tone="warn">浏览器查看模式：设备、配方保存与生产操作需要桌面后端。</Alert></div>}
              <Outlet />
            </ErrorBoundary>
          </section>
        </main>
      </div>
    </WindowFrame>
  );
}
