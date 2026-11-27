import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { FolderOpen, PackageOpen } from "lucide-react";
import { Alert, Button, Card, Stack, Text } from "qing-steel";
import { desktopAvailable, desktopCall } from "../lib/desktop";
import { errorText } from "../lib/errors";
import "./app-log.css";

type DiagnosticsResult = { path: string; notes: string[] };
type LogBatch = { cursor: number; lines: string[]; skipped: boolean };
const MAX_LINES = 200;

function RuntimeLogStream() {
  const desktop = desktopAvailable();
  const [lines, setLines] = useState<string[]>([]);
  const [error, setError] = useState("");
  const [paused, setPaused] = useState(false);
  const [skipped, setSkipped] = useState(false);
  const viewport = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const refresh = useRef<() => void>(() => {});

  useEffect(() => {
    if (!desktop) return;
    let cursor: number | null = null;
    let disposed = false;
    let busy = false;
    let timer: number | undefined;
    const visible = () => !disposed && !document.hidden && following.current;

    async function poll() {
      window.clearTimeout(timer);
      if (!visible() || busy) return;
      busy = true;
      try {
        const batch: LogBatch = await desktopCall("app_log_tail", { after: cursor });
        if (!visible()) return;
        cursor = batch.cursor;
        setError("");
        if (batch.lines.length || batch.skipped) {
          setSkipped(batch.skipped);
          setLines(previous => batch.skipped ? batch.lines.slice(-MAX_LINES) : previous.concat(batch.lines).slice(-MAX_LINES));
        }
      } catch (cause) {
        if (visible()) setError(errorText(cause));
      } finally {
        busy = false;
        if (visible()) timer = window.setTimeout(poll, 1000);
      }
    }

    refresh.current = poll;
    document.addEventListener("visibilitychange", poll);
    void poll();
    return () => {
      disposed = true;
      window.clearTimeout(timer);
      refresh.current = () => {};
      document.removeEventListener("visibilitychange", poll);
    };
  }, [desktop]);

  useLayoutEffect(() => {
    const el = viewport.current;
    if (el && following.current) el.scrollTop = el.scrollHeight;
  }, [lines]);

  function follow(atBottom: boolean) {
    if (following.current === atBottom) return;
    following.current = atBottom;
    setPaused(!atBottom);
    refresh.current();
  }

  function resume() {
    const el = viewport.current;
    if (el) el.scrollTop = el.scrollHeight;
    follow(true);
  }

  return <section className="runtime-stream" aria-labelledby="runtime-stream-title">
    <header>
      <h2 id="runtime-stream-title">运行日志流</h2>
      {paused ? <Button size="sm" variant="text" onClick={resume}>回到最新</Button>
        : <Text muted>{skipped ? "日志量较大，已跳至最近记录" : "最近 200 行"}</Text>}
    </header>
    {error && <Alert tone="warn">日志读取失败：{error}</Alert>}
    <div className="runtime-stream-viewport" ref={viewport} tabIndex={0} role="region" aria-label="日志内容"
      onScroll={e => {
        const el = e.currentTarget;
        follow(el.scrollHeight - el.clientHeight - el.scrollTop <= 4);
      }}>
      {lines.length ? <pre>{lines.join("\n")}</pre>
        : <Text muted>{desktop ? "等待运行日志…" : "在桌面客户端中查看实时日志。"}</Text>}
    </div>
  </section>;
}

export default function AppLogPage() {
  const [exporting, setExporting] = useState(false);
  const [result, setResult] = useState<DiagnosticsResult | null>(null);
  const [error, setError] = useState("");

  async function openDirectory() {
    setError("");
    try {
      await desktopCall("app_open_log_dir");
    } catch (cause) {
      setError(errorText(cause));
    }
  }

  async function exportDiagnostics() {
    setExporting(true);
    setResult(null);
    setError("");
    try {
      setResult(await desktopCall<DiagnosticsResult>("diagnostics_export"));
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setExporting(false);
    }
  }

  return <div className="runtime-log-page">
    <Card>
      <Stack direction="column" align="stretch" gap={12}>
        <Stack gap={8}>
          <Button leadingIcon={<FolderOpen size={16} />} onClick={openDirectory}>打开日志目录</Button>
          <Button variant="filled" leadingIcon={<PackageOpen size={16} />} loading={exporting} disabled={exporting} onClick={exportDiagnostics}>
            {exporting ? "正在导出…" : "导出诊断包"}
          </Button>
        </Stack>
        <Text muted>诊断包包含最近 24 小时的运行日志、PLC 通讯日志和检测记录，最多附带 20 件异常工件的原图。</Text>
        {result && <Alert tone={result.notes.length ? "warn" : "ok"} title="诊断包已导出">
          <div className="mono">{result.path}</div>
          {result.notes.length > 0 && <ul>{result.notes.map((note, index) => <li key={index}>{note}</li>)}</ul>}
        </Alert>}
        {error && <Alert tone="ng">{error}</Alert>}
      </Stack>
    </Card>
    <RuntimeLogStream />
  </div>;
}
