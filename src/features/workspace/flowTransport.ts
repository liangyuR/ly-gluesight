import { listen } from "@tauri-apps/api/event";
import type {
  BackupStatus, CacheStats, CoreInfo, CoreReloadFailed, EvictResult, ExecutionEvent, GraphDiagnostic, GraphDoc, LibraryRefresh,
  LibrarySettings, LibraryStatus, LoadedGraph, ManifestUpdated, OperatorManifestBundle, OutputInfo, PlanNode, RecentEntry,
  RecipeDirListing, RunOptions, RunOutputs, SubgraphDef, Transport, Unlisten,
} from "@lyflow/editor";
import { desktopCall } from "../../lib/desktop";

/** 流程调试页打开的测量图：示教走廊，或逐帧定向（搜索与回退两支在同一张图里）。 */
export type FlowVariant = "corridor" | "heading";

/** 候选里一（相机, 段）在示教件某个帧组上的测量图。 */
export interface FlowTarget { id: string; camera: string; segment: number; round: string; seq: number; variant: FlowVariant }

/** 编辑器的 graphPath：只用来区分打开的是哪一张，后端不读它。 */
export const flowGraphPath = (t: FlowTarget) => `gluesight-flow/${encodeURIComponent(t.id)}/${t.camera}/${t.segment}/${t.variant}/${t.round}/${t.seq}.lyflow.json`;

/** 打开的测量图与后端给的打开编号：之后的校验、编计划、运行与清缓存都带着它，切换后旧请求被拒。 */
export type OpenedFlow = LoadedGraph & { session: number };

/** 打开测量图：后端按候选配方合成这一（相机, 段）的有效参数，并记下这一帧原图供运行注入。 */
export const openFlow = (t: FlowTarget) => desktopCall<OpenedFlow>("flow_open", { ...t });

const bytes = async (command: string, args: Record<string, unknown>) => {
  const raw = await desktopCall<ArrayBuffer | Uint8Array>(command, args);
  return raw instanceof ArrayBuffer ? raw : raw.buffer.slice(raw.byteOffset, raw.byteOffset + raw.byteLength) as ArrayBuffer;
};
const unsupported = (what: string) => Promise.reject(new Error(`流程调试页不支持${what}`));
const noop = async () => {};

/**
 * 嵌入编辑器的 Transport（LF-6）：只开放调参与运行。图来自 `flow_open` 的结果，运行、编计划与清缓存由后端注入同一份调试帧；
 * 最近文件、备份、配方、库、导入导出都是空实现或直接拒绝。
 */
export class FlowTransport implements Transport {
  readonly kind = "tauri" as const;
  constructor(private readonly path: string, private readonly loaded: OpenedFlow) {}

  getManifest() { return desktopCall<OperatorManifestBundle>("flow_manifest"); }
  getCoreInfo() { return desktopCall<CoreInfo>("flow_core_info"); }
  async loadGraph(path: string): Promise<LoadedGraph> {
    if (path !== this.path) throw new Error("流程调试页只能打开当前选中的测量图");
    return structuredClone(this.loaded);
  }
  saveGraph() { return unsupported("保存图：专家参数请用“保存为本相机专家参数”写进候选"); }
  validateGraph(doc: GraphDoc, _graphPath: string | null, params?: Record<string, unknown>) {
    return desktopCall<GraphDiagnostic[]>("flow_validate", { session: this.loaded.session, doc, params: params ?? null });
  }
  async planGraph(doc: GraphDoc, _graphPath: string | null, targets?: string[], params?: Record<string, unknown>): Promise<PlanNode[]> {
    const out = await desktopCall<{ cacheKey?: unknown }[]>("flow_plan", { session: this.loaded.session, doc, targets: targets ?? null, params: params ?? null });
    // 校验没过时回的是诊断数组，靠有没有 cacheKey 区分
    return out.some(node => typeof node.cacheKey !== "string") ? [] : out as PlanNode[];
  }
  clearCache() { return desktopCall<void>("flow_clear_cache"); }
  evictCache(doc: GraphDoc, _graphPath: string | null, nodeIds: string[], downstream: boolean, params?: Record<string, unknown>) {
    return desktopCall<EvictResult>("flow_evict_cache", { session: this.loaded.session, doc, nodeIds, downstream, params: params ?? null });
  }
  cacheStats() { return desktopCall<CacheStats>("flow_cache_stats"); }
  runGraph(doc: GraphDoc, _graphPath: string | null, options?: RunOptions) {
    return desktopCall<string>("flow_run", {
      session: this.loaded.session, doc, targets: options?.targets ?? null, isolate: options?.isolate ?? null, force: options?.force ?? null,
      mode: options?.mode ?? "full", previewMaxPoints: options?.previewMaxPoints ?? null, params: options?.params ?? null,
    });
  }
  cancelRun(runId: string) { return desktopCall<void>("flow_cancel", { runId }); }
  getOutputInfo(runId: string, nodeId: string) { return desktopCall<OutputInfo[]>("flow_output_info", { runId, nodeId }); }
  getRunOutputs(runId: string) { return desktopCall<RunOutputs>("flow_run_outputs", { runId }); }
  getOutputImage(runId: string, nodeId: string, port: string, level: number, row: number, rows: number) {
    return bytes("flow_output_image", { runId, nodeId, port, level, row, rows });
  }
  getOutputCloud() { return unsupported("点云输出"); }
  getOutputTensor() { return unsupported("张量输出"); }
  getOutputIndices() { return unsupported("索引输出"); }
  onExecutionEvent(cb: (e: ExecutionEvent) => void): Promise<Unlisten> {
    return listen<ExecutionEvent>("flow-execution-event", e => cb(e.payload));
  }
  async onManifestUpdated(_cb: (e: ManifestUpdated) => void): Promise<Unlisten> { return () => {}; }
  async onCoreReloadFailed(_cb: (e: CoreReloadFailed) => void): Promise<Unlisten> { return () => {}; }

  async getRecentFiles(): Promise<RecentEntry[]> { return []; }
  async pushRecentFile(): Promise<RecentEntry[]> { return []; }
  writeBackup = noop;
  async backupStatus(): Promise<BackupStatus> { return { exists: false, newer: false, backupModified: null, fileModified: null }; }
  readBackup() { return unsupported("读备份"); }
  discardBackup = noop;
  async listRecipeDir(): Promise<RecipeDirListing> { return { exists: false, files: [] }; }
  readRecipeFile() { return unsupported("读配方文件"); }
  writeRecipeFile() { return unsupported("写配方文件"); }
  deleteRecipeFile = noop;
  renameRecipeFile() { return unsupported("改配方文件名"); }
  writeFileBytes() { return unsupported("导出文件"); }
  async getLibraryStatus(): Promise<LibraryStatus> { return { dirs: [], count: 0, problems: [] }; }
  refreshLibrary(): Promise<LibraryRefresh> { return unsupported("算子库"); }
  async getLibrarySettings(): Promise<LibrarySettings> { return { defaultDir: "", extraDirs: [], envDirs: [], editable: false }; }
  setLibraryDirs(): Promise<LibraryRefresh> { return unsupported("算子库"); }
  saveAsLibrary(): Promise<LibraryStatus> { return unsupported("保存到库"); }
  async getLibraryDefinition(): Promise<SubgraphDef | null> { return null; }
  importGraph(): Promise<GraphDoc> { return unsupported("导入图"); }
}
