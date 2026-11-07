import { useParams } from "react-router-dom";

/**
 * 配方页按地址打开（不设全局“当前配方”）：页面里写步骤路径（`/recipe/capture`、`/recipe/teach?camera=…`），
 * 链接时换成带配方编号的地址 `/recipe/<编号>/capture`。配方库本身是 `/recipe`。
 */
export function recipeHref(id: string, step: string): string {
  return step.replace(/^\/recipe(?=[/?]|$)/, `/recipe/${encodeURIComponent(id)}`);
}

/** 当前地址对应的步骤路径（去掉配方编号）；不在配方步骤页时原样返回。 */
export function stepPathOf(pathname: string): string {
  const match = pathname.match(/^\/recipe\/[^/]+(\/.+)$/);
  return match ? "/recipe" + match[1] : pathname;
}

/** 本页配方的步骤链接；不在配方步骤页时回配方库。 */
export function useRecipeHref(): (step: string) => string {
  const { recipeId } = useParams();
  return step => recipeId ? recipeHref(recipeId, step) : "/recipe";
}
