import { useEffect, useState, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Copy, Minus, Square, X } from "lucide-react";
import { useSnackbar } from "qing-steel";
import { desktopAvailable } from "../lib/desktop";
import { errorText } from "../lib/errors";
import "./window-frame.css";

export default function WindowFrame({ children }: { children: ReactNode }) {
  const [nativeWindow] = useState(() => desktopAvailable() ? getCurrentWindow() : null);
  const [maximized, setMaximized] = useState(false);
  const [focused, setFocused] = useState(true);
  const { show } = useSnackbar();

  useEffect(() => {
    if (!nativeWindow) return;
    let disposed = false, resizeVersion = 0, focusVersion = 0;
    const unlisteners: (() => void)[] = [];
    const subscribe = (subscription: Promise<() => void>) => {
      void subscription.then(unlisten => {
        if (disposed) unlisten();
        else unlisteners.push(unlisten);
      }).catch(error => console.error("窗口状态监听失败", error));
    };
    const refreshMaximized = async () => {
      const version = ++resizeVersion;
      const value = await nativeWindow.isMaximized();
      if (!disposed && version === resizeVersion) setMaximized(value);
    };
    subscribe(nativeWindow.onResized(() => {
      void refreshMaximized().catch(error => console.error("读取窗口状态失败", error));
    }));
    subscribe(nativeWindow.onFocusChanged(({ payload }) => {
      focusVersion++;
      if (!disposed) setFocused(payload);
    }));
    void refreshMaximized().catch(error => console.error("读取窗口状态失败", error));
    const initialFocusVersion = focusVersion;
    void nativeWindow.isFocused().then(value => {
      if (!disposed && focusVersion === initialFocusVersion) setFocused(value);
    }).catch(error => console.error("读取窗口焦点失败", error));
    return () => {
      disposed = true;
      unlisteners.forEach(unlisten => unlisten());
    };
  }, [nativeWindow]);

  const run = (action: () => Promise<void>) => {
    void action().catch(error => show({ text: `窗口操作失败：${errorText(error)}`, tone: "ng" }));
  };
  const maximizeLabel = maximized ? "还原窗口" : "最大化窗口";

  return (
    <div className={`app-frame${maximized ? " is-maximized" : ""}${focused ? "" : " is-unfocused"}`}>
      <header className="window-titlebar" aria-label="应用标题栏">
        <div className="window-drag-region"
          onMouseDown={event => {
            if (!nativeWindow || event.button !== 0) return;
            if (event.detail === 2) run(() => nativeWindow.toggleMaximize());
            else run(() => nativeWindow.startDragging());
          }}>
          <div className="window-brand">
            <img src="/app-icon.png" alt="" draggable={false} />
            <strong>GlueSight</strong>
          </div>
        </div>
        {nativeWindow ? <div className="window-controls" role="group" aria-label="窗口控制">
          <button type="button" className="window-control" title="最小化窗口" aria-label="最小化窗口" onClick={() => run(() => nativeWindow.minimize())}><Minus aria-hidden="true" size={15} /></button>
          <button type="button" className="window-control" title={maximizeLabel} aria-label={maximizeLabel} onClick={() => run(() => nativeWindow.toggleMaximize())}>{maximized ? <Copy aria-hidden="true" size={13} /> : <Square aria-hidden="true" size={13} />}</button>
          <button type="button" className="window-control window-close" title="关闭窗口" aria-label="关闭窗口" onClick={() => run(() => nativeWindow.close())}><X aria-hidden="true" size={17} /></button>
        </div> : <span className="window-browser-label">浏览器预览</span>}
      </header>
      {children}
    </div>
  );
}
