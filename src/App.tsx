import { Navigate, Route, Routes } from "react-router-dom";
import { SnackbarProvider } from "qing-steel";
import AppLayout from "./layout/AppLayout";
import { navItems, pageRoutes } from "./layout/nav";
import HistoryDetailPage from "./pages/HistoryDetailPage";
import { WorkspaceProvider } from "./features/workspace/context";
import { LicenseProvider } from "./features/licensing/context";

export default function App() {
  return (
    <Routes>
      <Route element={<SnackbarProvider><LicenseProvider><WorkspaceProvider><AppLayout /></WorkspaceProvider></LicenseProvider></SnackbarProvider>}>
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
