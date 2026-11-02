import React, { useEffect } from "react";
import ReactDOM from "react-dom/client";
import { HashRouter } from "react-router-dom";
import { ThemeProvider } from "qing-steel";
import App from "./App";
import { desktopCall } from "./lib/desktop";
import "qing-steel/react.css";
import "./styles.css";

/** 首屏渲染完通知后端：显示主窗口、关闭启动画面。 */
function StartupReady() {
  useEffect(() => { desktopCall("startup_ready").catch(() => undefined); }, []);
  return null;
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider defaultTheme="dark" className="app-theme">
      <HashRouter>
        <App />
      </HashRouter>
    </ThemeProvider>
    <StartupReady />
  </React.StrictMode>,
);
