import { useEffect, type ComponentType } from "react";
import { Link, Navigate, useLocation, useParams } from "react-router-dom";
import { Button } from "qing-steel";
import { useWorkspace } from "./context";
import { Notice } from "./components";
import { recipeHref } from "./recipeRoutes";

/**
 * 配方步骤页的入口：打开哪个配方由地址里的编号决定（`/recipe/<编号>/…`），没有全局“选中配方”。
 * 进入时按编号打开这个配方的候选；另一个配方还有没保存的修改时不切换，提示回去保存或撤销；
 * 另一个配方还有操作或保存在途时等它（连同页面卸载时的续存）做完再切换，免得切换打断续存、丢掉最后一次修改。
 */
function RecipeRoute({ children }: { children: React.ReactNode }) {
  const { recipeId = "" } = useParams();
  const { selectedId, select, dirty, frameDirty, navigationDirty, segmentDirty, busy } = useWorkspace();
  const unsaved = dirty || frameDirty || navigationDirty || segmentDirty;
  const other = selectedId != null && selectedId !== recipeId;
  useEffect(() => {
    if (recipeId && selectedId !== recipeId && !unsaved && !busy) void select(recipeId);
  }, [recipeId, selectedId, unsaved, busy, select]);
  if (other && unsaved) {
    return <div className="wp-page"><Notice title={`配方 ${selectedId} 有未保存的修改`} tone="warn">
      先回到 {selectedId} 保存或撤销修改，再打开 {recipeId}。
      <Button asChild size="sm"><Link to={recipeHref(selectedId, "/recipe/progress")}>回到 {selectedId}</Link></Button>
    </Notice></div>;
  }
  // 还没换到地址里的配方时不渲染页面，免得先露出上一个配方的内容
  if (selectedId !== recipeId) return <div className="wp-page"><p className="muted">正在打开配方 {recipeId}…</p></div>;
  return <>{children}</>;
}

/** 原段列表页地址（/recipe/<编号>/segments）转到分段与示教页，带上查询与定位状态（历史件“调整分段”的定位）。 */
export function SegmentsRedirect() {
  const { recipeId = "" } = useParams();
  const { search, state } = useLocation();
  return <Navigate replace to={{ pathname: recipeHref(recipeId, "/recipe/teach"), search }} state={state} />;
}

/** 把配方步骤页包上按地址打开配方的入口。 */
export function inRecipe(Page: ComponentType): ComponentType {
  const Wrapped = () => <RecipeRoute><Page /></RecipeRoute>;
  Wrapped.displayName = `InRecipe(${Page.displayName ?? Page.name})`;
  return Wrapped;
}
