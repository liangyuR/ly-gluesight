/** 历史件与导入件的界面标识（RB-4）：时间与 SN，不显示轮次编号。 */
export const pieceTime = (ts: number) => new Date(ts).toLocaleString("zh-CN");

export function pieceLabel(piece: { sn: string | null; ts: number | null }) {
  return [piece.ts != null ? pieceTime(piece.ts) : null, piece.sn ? `SN ${piece.sn}` : null].filter(Boolean).join(" · ") || "来源未记录";
}
