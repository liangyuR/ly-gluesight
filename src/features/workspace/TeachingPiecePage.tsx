import { useState } from "react";
import { Link } from "react-router-dom";
import { ArrowRight } from "lucide-react";
import { Button, Dialog } from "qing-steel";
import ContextHelp from "../cycle/components/ContextHelp";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Notice, Panel, WorkspaceBar, WorkspaceEmpty } from "./components";
import { FrameBrowser } from "./FrameBrowser";
import HistoryPicker from "./HistoryPicker";
import { pieceTime } from "./pieces";
import type { HistoryPick, WorkspaceView } from "./types";

/** 设示教件会清空的内容（RB-2、RB-8）：只列候选里实际有的。 */
export function clearedByTeachingPiece(view: WorkspaceView): string[] {
  const { workspace, plan } = view;
  const doc = workspace.doc;
  const confirmed = Object.values(workspace.captureReview?.rounds ?? {}).some(review => review.status === "confirmed");
  const corridors = new Set([
    ...plan.cameraTeaching.flatMap(item => item.segments.map(segment => `${item.camera}#${segment.segment}`)),
    ...(doc.inspect ?? []).flatMap(item => (item.segments ?? []).filter(segment => segment.corridor.length > 0).map(segment => `${item.camera}#${segment.segment}`)),
  ]).size;
  const others = plan.pieces.filter(piece => piece.role !== "teaching").length;
  const segments = doc.segments ?? [];
  const calibrated = !!doc.partCalib || workspace.calibration.rounds.length > 0
    || segments.some(segment => segment.expectedFrames != null || segment.calib != null || segment.offsetRef?.xRef != null);
  return [
    ...(workspace.teachingPiece || confirmed ? ["示教件的分段边界与其余各件已确认的边界"] : []),
    ...(corridors ? [`全部走廊与示教帧（${corridors} 条）`] : []),
    ...(others ? [`全部标定件与标准件（${others} 件），需在多轮标定重新选件`] : []),
    ...(calibrated ? ["标定结果：各段预期帧数 N_exp、标定统计、整件标定与胶偏参考 x_ref"] : []),
  ];
}

/**
 * 示教件（RB-1，路由 /recipe/capture）：从本配方的历史件里选一件，分段边界与全部走廊示教帧都只来自它，它也计入标定件。
 * 已有示教件或候选里有继承的走廊、标定结果时，先列出将清空的内容（RB-2、RB-8），确认后才替换。
 */
export default function TeachingPiecePage() {
  const { data, doc, cameras, busy, dirty, frameDirty, act, setError } = useWorkspace();
  const [confirming, setConfirming] = useState<{ recordId: number; done: (ok: boolean) => void } | null>(null);
  if (!doc || !data) return <WorkspaceEmpty />;
  const { workspace, plan } = data;
  const piece = workspace.teachingPiece;
  const groupCount = plan.pieces.find(item => item.role === "teaching")?.groupCount ?? 0;
  const locked = busy || dirty || frameDirty || !!workspace.pending;
  const nameOf = (id: string) => cameras.find(camera => camera.id === id)?.name ?? id;
  const cleared = clearedByTeachingPiece(data);
  const apply = async (recordId: number) => !!await act(() => workspaceApi.setTeachingPiece(doc.id, workspace.revision, recordId), "已设为示教件，请到分段确认边界");
  const choose = async ([pick]: HistoryPick[]) => {
    if (!pick) return false;
    if (!cleared.length) return apply(pick.recordId);
    return new Promise<boolean>(done => setConfirming({ recordId: pick.recordId, done }));
  };
  const close = (ok: boolean) => { confirming?.done(ok); setConfirming(null); };
  const confirm = async () => {
    if (!confirming) return;
    close(await apply(confirming.recordId));
  };
  return <div className="wp-page"><WorkspaceBar />
    <Panel title="示教件" help="分段边界与各段走廊示教帧都只来自示教件；示教件也计入标定件，在走廊示教完成后由多轮标定加入">
      {piece ? <>
        <div className="wp-capture-summary" aria-label="当前示教件">
          <span className="wp-capture-chip"><small>SN</small><strong className="mono">{piece.sn}</strong></span>
          <span className="wp-capture-chip"><small>时间</small><strong>{pieceTime(piece.ts)}</strong></span>
          <span className="wp-capture-chip"><small>帧组</small><strong>{groupCount} 个</strong></span>
          <Link className="wp-text-link" to="/recipe/segments">前往分段<ArrowRight size={12} aria-hidden="true" /></Link>
        </div>
        <FrameBrowser key={piece.roundId} source={piece.roundId} label="示教件" groupCount={groupCount}
          cameras={plan.cameras.map(id => ({ id, name: nameOf(id) }))}
          read={(camera, seq) => workspaceApi.captureFrameImage(piece.roundId, plan.cameras[camera], seq)} />
      </> : <Notice title="还没有示教件">从下方历史件里选一件完整的录制件设为示教件：分段与走廊示教都在它的图像上进行。没有示教件的候选不能发布。</Notice>}
    </Panel>
    <HistoryPicker recipeId={doc.id} mode="single" title="选示教件" disabled={locked} nameOf={nameOf} onError={setError}
      confirmLabel={() => "设为示教件"} onConfirm={choose}
      help={<ContextHelp label="选示教件说明">
        <p>维护模式或生产录制的完整件，导入后作为本候选的示教件（原图硬链接，不停线）。</p>
        <p>可选用：原图完整（无缺帧、无录制队满未录）、三台计划相机齐全且设备配置与当前一致、没有提前结束、型号属于本候选；已选作示教件或标定件的标“已使用”。</p>
        <p>选好后看一遍各相机的帧组，确认胶路完整、图像清晰再设为示教件。</p>
      </ContextHelp>} />
    {confirming && <Dialog open title={piece ? "更换示教件" : "设为示教件"} onOpenChange={open => { if (!open) close(false); }}
      footer={<><Button disabled={busy} onClick={() => close(false)}>取消</Button><Button variant="filled" danger disabled={busy} onClick={() => void confirm()}>清空并设为示教件</Button></>}>
      <div className="wp-stack">
        <p>以下由示教件图像得出的内容将清空，不按图像迁移：</p>
        <ul aria-label="将清空的内容">{cleared.map(item => <li key={item}>{item}</li>)}</ul>
        <p className="muted">保留段列表结构（段的数量、顺序、类型、检测相机、检测项与容差）与配方级参数；进入分段时按现有段结构按比例给出初始边界，须重新确认。</p>
      </div>
    </Dialog>}
  </div>;
}
