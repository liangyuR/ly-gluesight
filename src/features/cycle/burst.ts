import { useEffect, useState } from "react";
import type { PathSegment } from "../plan/types";
import { recipeApi } from "./api";

/**
 * 段配方的段列表，在线检测的整件布置用。在途件（PartView.burst）与结果（ResultView.burst）只带区域，不带段，
 * 所以按配方 id 取配方库里的段；revisionId 变了（重新发布）就重取。未启用、取不到或旧拍照点配方时为空数组。
 */
export function useBurstSegments(recipeId: string | null | undefined, revisionId: string | null | undefined, enabled: boolean): PathSegment[] {
  const key = enabled && recipeId ? `${recipeId}:${revisionId ?? ""}` : "";
  const [state, setState] = useState<{ key: string; segments: PathSegment[] } | null>(null);
  useEffect(() => {
    if (!key || !recipeId) return;
    let alive = true;
    recipeApi.doc(recipeId).then(doc => alive && setState({ key, segments: doc.segments ?? [] })).catch(() => alive && setState({ key, segments: [] }));
    return () => { alive = false; };
  }, [key, recipeId]);
  return state?.key === key ? state.segments : [];
}
