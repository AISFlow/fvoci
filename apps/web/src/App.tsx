import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { SetupGuard } from "@/components/setup-guard";
import { DocumentPage } from "@/pages/DocumentPage";
import { HomePage } from "@/pages/HomePage";
import { LoginPage } from "@/pages/LoginPage";
import { SetupPage } from "@/pages/SetupPage";
import { WikiPage } from "@/pages/WikiPage";
import { WorkspaceLayout } from "@/pages/WorkspaceLayout";
import { WorkspaceSettingsPage } from "@/pages/WorkspaceSettingsPage";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      refetchOnWindowFocus: false,
    },
  },
});

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <Routes>
          <Route path="/setup" element={<SetupPage />} />
          <Route
            path="/login"
            element={
              <SetupGuard>
                <LoginPage />
              </SetupGuard>
            }
          />
          <Route
            path="/"
            element={
              <SetupGuard>
                <HomePage />
              </SetupGuard>
            }
          />
          <Route
            path="/w/:slug"
            element={
              <SetupGuard>
                <WorkspaceLayout />
              </SetupGuard>
            }
          >
            <Route path="wiki" element={<WikiPage />} />
            <Route path="settings" element={<WorkspaceSettingsPage />} />
            <Route path=":ref" element={<DocumentPage />} />
          </Route>
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </BrowserRouter>
    </QueryClientProvider>
  );
}
