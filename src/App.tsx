import { Navigate, Route, Routes } from "react-router-dom";
import { lazy, Suspense } from "react";
import AppLayout from "./layout/AppLayout";
import { navItems } from "./layout/nav";
import HistoryDetailPage from "./pages/HistoryDetailPage";
import { WorkspaceProvider } from "./features/workspace/context";
const WorkflowPreviewPage = lazy(() => import("./pages/WorkflowPreviewPage"));

export default function App() {
  return (
    <Routes>
      <Route path="/workflow/:view?" element={<Suspense fallback={<div className="empty" style={{ height: "100%" }}>正在加载操作流程…</div>}><WorkflowPreviewPage /></Suspense>} />
      <Route element={<WorkspaceProvider><AppLayout /></WorkspaceProvider>}>
        <Route index element={<Navigate to={navItems[0].path} replace />} />
        {navItems.map(({ path, element: Page }) => (
          <Route key={path} path={path} element={<Page />} />
        ))}
        <Route path="/history/:id" element={<HistoryDetailPage />} />
        <Route path="*" element={<Navigate to={navItems[0].path} replace />} />
      </Route>
    </Routes>
  );
}
