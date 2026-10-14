import { Navigate, Route, Routes } from "react-router-dom";
import AppLayout from "./layout/AppLayout";
import { navItems, pageRoutes } from "./layout/nav";
import HistoryDetailPage from "./pages/HistoryDetailPage";
import { WorkspaceProvider } from "./features/workspace/context";

export default function App() {
  return (
    <Routes>
      <Route element={<WorkspaceProvider><AppLayout /></WorkspaceProvider>}>
        <Route index element={<Navigate to={navItems[0].path} replace />} />
        {pageRoutes.map(({ path, element: Page }) => (
          <Route key={path} path={path} element={<Page />} />
        ))}
        <Route path="/history/:id" element={<HistoryDetailPage />} />
        <Route path="*" element={<Navigate to={navItems[0].path} replace />} />
      </Route>
    </Routes>
  );
}
