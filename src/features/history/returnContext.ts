/** 离开历史详情（去分段页）时要带回来的复测上下文：复测配方、选中的相机与帧组、选着的对照、是否显示原始记录。 */
export interface HistoryReturnContext {
  candidate: string | null;
  camera: number;
  seq: number;
  comparison: string | null;
  original: boolean;
}

export function historyReturnSearch(context: HistoryReturnContext): string {
  const search = new URLSearchParams({ camera: String(context.camera), seq: String(context.seq) });
  if (context.candidate) search.set("candidate", context.candidate);
  if (context.comparison) search.set("comparison", context.comparison);
  if (context.original) search.set("view", "original");
  return "?" + search.toString();
}

/** 从地址读回复测上下文；相机、帧组不在本件范围内时回到第一台相机、第一个帧组（换件进来的地址不带这些参数）。 */
export function historyReturnContext(search: URLSearchParams, cameraCount: number, seqs: number[]): HistoryReturnContext {
  const camera = Number(search.get("camera")), seq = Number(search.get("seq"));
  return {
    candidate: search.get("candidate"),
    camera: Number.isInteger(camera) && camera >= 0 && camera < cameraCount ? camera : 0,
    seq: search.has("seq") && seqs.includes(seq) ? seq : seqs[0] ?? 1,
    comparison: search.get("comparison"),
    original: search.get("view") === "original",
  };
}
