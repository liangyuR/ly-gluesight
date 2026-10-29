import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { ChevronsUpDown, FolderKanban } from "lucide-react";
import { Button, Menu, type MenuEntry } from "qing-steel";
import { projectApi, type ProjectEntry, type ProjectMeta } from "../../lib/project";
import { desktopAvailable } from "../../lib/desktop";
import { errorText } from "../../lib/errors";
import { onProjectsChanged } from "./hooks";
import SwitchProjectDialog from "./SwitchProjectDialog";
import "./project.css";

export default function ProjectSwitcher({ collapsed }: { collapsed: boolean }) {
  const navigate = useNavigate();
  const [current, setCurrent] = useState<ProjectMeta | null>(null);
  const [currentError, setCurrentError] = useState("");
  const [entries, setEntries] = useState<ProjectEntry[]>([]);
  const [listError, setListError] = useState("");
  const [target, setTarget] = useState<ProjectMeta | null>(null);
  const serial = useRef(0);
  const load = useCallback(() => {
    if (!desktopAvailable()) { setCurrentError("需要桌面后端"); return; }
    const request = ++serial.current;
    projectApi.current().then(meta => { if (request === serial.current) { setCurrent(meta); setCurrentError(""); } })
      .catch(e => { if (request === serial.current) setCurrentError(errorText(e)); });
    projectApi.list().then(list => { if (request === serial.current) { setEntries(list); setListError(""); } })
      .catch(e => { if (request === serial.current) setListError(errorText(e)); });
  }, []);
  useEffect(() => {
    load();
    const stop = onProjectsChanged(load);
    return () => { serial.current++; stop(); };
  }, [load]);
  const others = entries.filter((entry): entry is ProjectEntry & { meta: ProjectMeta } => !entry.current && !!entry.meta);
  const items: MenuEntry[] = [
    ...(others.length
      ? [{ type: "label" as const, id: "switch-label", label: "切换到" }, ...others.map(entry => ({ id: "project:" + entry.id, label: `${entry.meta.name} · ${entry.id}`, onSelect: () => setTarget(entry.meta) }))]
      : [{ type: "label" as const, id: "none", label: listError ? "项目列表读取失败" : "没有其他可用项目" }]),
    { type: "separator", id: "separator" },
    { id: "manage", label: "管理项目…", onSelect: () => navigate("/projects") },
  ];
  const name = current?.name ?? (currentError ? "项目未知" : "读取中…");
  const hint = current ? `当前项目：${current.name}（${current.id}）` : currentError ? `当前项目读取失败：${currentError}` : "正在读取当前项目";
  const trigger = <Button variant="tonal" className="project-card" aria-label={hint + "，打开项目菜单"} title={hint} onPointerDown={load} onKeyDown={e => { if (["Enter", " ", "ArrowDown"].includes(e.key)) load(); }}>
    <FolderKanban size={18} aria-hidden="true" />
    {!collapsed && <span className="project-card-text"><strong>{name}</strong><span className="mono">{current?.id ?? (currentError || " ")}</span></span>}
    {!collapsed && <ChevronsUpDown size={16} aria-hidden="true" className="project-card-chevron" />}
  </Button>;
  return <>
    <div className="project-switcher"><Menu trigger={trigger} items={items} align="start" label="项目菜单" /></div>
    {target && <SwitchProjectDialog target={target} onClose={() => setTarget(null)} />}
  </>;
}
