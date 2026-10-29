import { Button } from "qing-steel";
import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { errorText } from "../../lib/errors";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Notice, Panel } from "./components";
import NavigationMap from "./navigation/NavigationMap";
import type { NavigationMapData } from "./navigation/types";
import type { PathSegment } from "../plan/types";

export interface WorkspaceNavigationProps {
  segments: Pick<PathSegment, "index" | "kind">[];
  ranges: [number, number][] | null;
  seq: number;
  currentSegment?: number;
  frameStatus: "confirmed" | "draft" | "unavailable";
  editable?: boolean;
  locked?: string;
  onNavigate: (segment: number, seq: number) => void;
}

export default function WorkspaceNavigation(props: WorkspaceNavigationProps) {
  const { data } = useWorkspace();
  if (!data) return null;
  return <NavigationContent key={data.workspace.doc.id} {...props} />;
}

function NavigationContent({ editable = false, locked = "", ...props }: WorkspaceNavigationProps) {
  const { data, busy, dirty, frameDirty, act, navigationDraft, setNavigationDraft } = useWorkspace();
  const id = data!.workspace.doc.id;
  const navigation = data!.navigation;
  const map = navigation?.map ?? null;
  const [picture, setPicture] = useState<{ key: string; url: string; error: string }>({ key: "", url: "", error: "" });
  const [retry, setRetry] = useState(0);
  const imageKey = JSON.stringify([id, map?.image.id]);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const imageId = map?.image.id;
  useEffect(() => {
    if (!imageId || !navigation?.imageAvailable) return;
    let alive = true;
    let url = "";
    void workspaceApi.navigationImage(id, imageId).then(bytes => {
      if (!alive) return;
      const buffer = bytes instanceof ArrayBuffer ? bytes : Uint8Array.from(bytes).buffer;
      url = URL.createObjectURL(new Blob([buffer], { type: "image/png" }));
      setPicture({ key: imageKey, url, error: "" });
    }).catch(error => { if (alive) setPicture({ key: imageKey, url: "", error: errorText(error) }); });
    return () => { alive = false; if (url) URL.revokeObjectURL(url); };
  }, [id, imageId, imageKey, navigation?.imageAvailable, retry]);

  const reason = locked || (data!.workspace.pending ? "候选正在等待生效" : busy ? "正在保存，请稍候" : dirty || frameDirty ? "请先保存候选配置和走廊修改" : "");
  const revision = navigationDraft?.revision ?? data!.workspace.revision;
  const changed = (draft: NavigationMapData | null) => {
    setNavigationDraft(draft ? { map: draft, revision } : null);
  };
  const save = async (draft: NavigationMapData, confirm: boolean) => {
    if (reason) return false;
    const updated = { ...draft, structureRevision: navigation?.structureRevision ?? draft.structureRevision };
    const result = await act(() => workspaceApi.navigationSave(id, revision, updated, confirm), confirm ? "导航图已保存，空间对应已确认" : "导航图草稿已保存");
    if (!result) return false;
    setNavigationDraft(null);
    return true;
  };
  const importImage = async (file: File) => {
    if (reason) return;
    if (!/\.(png|jpe?g|bmp|tiff?)$/i.test(file.name) || !file.size || file.size > 32 * 1024 * 1024) throw new Error("请选择不超过 32 MiB 的 PNG、JPEG、BMP 或 TIFF 图片");
    const importedRevision = data!.workspace.revision;
    const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
    if (!alive.current) return;
    const result = await act(() => workspaceApi.navigationImport(id, importedRevision, bytes), "工件图已导入，请绘制胶路并标记采集范围");
    if (result) setNavigationDraft(null);
  };
  const remove = async () => {
    if (reason) return;
    const result = await act(() => workspaceApi.navigationDelete(id, revision), "候选导航图已移除，发布快照保留");
    if (result) setNavigationDraft(null);
  };
  const error = navigation?.error || (picture.key === imageKey ? picture.error : "");
  const imageReady = !!navigation?.imageAvailable && picture.key === imageKey && !!picture.url && !error;
  return <div className="workspace-navigation" onKeyDown={event => event.stopPropagation()}>
    {error && <Notice title="导航资料不可用" tone="warn">{error} <Button variant="text" size="sm" onClick={() => setRetry(value => value + 1)}>重试读图</Button></Notice>}
    {!editable && !map && <section className="navigation-map" aria-label="工件胶路导航图"><strong>工件胶路导航图</strong><p className="muted">尚未添加导航图，可在<Link to="/recipe/segments">分段页</Link>导入工件图并标注胶路。</p></section>}
    {(editable || map) && <NavigationMap {...props} ranges={imageReady ? props.ranges : null} map={map}
      imageUrl={picture.key === imageKey && navigation?.imageAvailable ? picture.url : ""}
      editable={editable} disabled={!!reason} disabledReason={reason}
      initialDraft={navigationDraft?.map} onDraftChange={changed}
      onSave={save} onImport={importImage} onRemove={remove} />}
  </div>;
}

export function NavigationPublication() {
  const { data } = useWorkspace();
  const navigation = data?.navigation;
  const map = navigation?.map;
  const spatialReady = !!map?.confirmed && navigation?.imageAvailable && !navigation.error;
  const teaching = data?.workspace.teachingPiece;
  const segments = data?.plan.segments ?? [];
  const piece = data?.plan.pieces.find(item => item.roundId === teaching?.roundId);
  const review = teaching ? data?.workspace.captureReview?.rounds[teaching.roundId] : undefined;
  const framesReady = !!teaching && (segments.length === 1 ? (piece?.groupCount ?? 0) > 0
    : segments.length > 1 && review?.status === "confirmed" && review.groupCount > 0 && review.cuts.length === segments.length - 1
      && review.cuts.every((cut, index) => Number.isInteger(cut) && cut > (review.cuts[index - 1] ?? 0) && cut < review.groupCount));
  return <Panel title="导航辅助资料">
    <p>{!map ? "候选未配置导航图" : !spatialReady ? "地图尚未确认或图片不可用，不提供帧定位" : framesReady ? "空间对应已确认，发布时冻结独立辅助快照" : "空间对应已确认，但示教件范围未确认，暂不可冻结导航快照"}</p>
    {navigation?.error && <p className="c-warn">{navigation.error}</p>}
    {navigation?.published && <p role="status">{navigation.published.detail}</p>}
    <p className="muted">辅助资料与检测包独立；缺失或损坏不影响生产。<Link to="/recipe/segments">编辑导航图</Link></p>
  </Panel>;
}
