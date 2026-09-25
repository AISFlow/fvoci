import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { SetupGuard } from "@/components/setup-guard";
import { HomePage } from "@/pages/HomePage";
import { LoginPage } from "@/pages/LoginPage";
import { SetupPage } from "@/pages/SetupPage";
import { SearchPage } from "@/pages/SearchPage";
import { ProjectsPage } from "@/pages/ProjectsPage";
import { ProjectTasksPage } from "@/pages/ProjectTasksPage";
import { TrashPage } from "@/pages/TrashPage";
import { WikiPage } from "@/pages/WikiPage";
import { WorkspaceLayout } from "@/pages/WorkspaceLayout";
import { WorkspaceRefPage } from "@/pages/WorkspaceRefPage";
import { WorkspaceSettingsPage } from "@/pages/WorkspaceSettingsPage";
import { NotificationsPage } from "@/pages/NotificationsPage";
import { InvitePage } from "@/pages/InvitePage";
import { AttachmentViewPage } from "@/pages/AttachmentViewPage";
import { ResetPasswordPage } from "@/pages/ResetPasswordPage";
import { PublicSharePage } from "@/pages/PublicSharePage";
import { WorkspaceHomePage } from "@/pages/WorkspaceHomePage";

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
          {/* Public share reader: no session and no setup guard (it must not redirect to /login). */}
          <Route path="/s/:token" element={<PublicSharePage />} />
          <Route
            path="/invite/:token"
            element={
              <SetupGuard>
                <InvitePage />
              </SetupGuard>
            }
          />
          <Route
            path="/login"
            element={
              <SetupGuard>
                <LoginPage />
              </SetupGuard>
            }
          />
          <Route
            path="/reset-password"
            element={
              <SetupGuard>
                <ResetPasswordPage />
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
            <Route index element={<WorkspaceHomePage />} />
            <Route path="projects" element={<ProjectsPage />} />
            <Route path="wiki" element={<WikiPage />} />
            <Route path="search" element={<SearchPage />} />
            <Route path="trash" element={<TrashPage />} />
            <Route path="settings" element={<WorkspaceSettingsPage />} />
            <Route path="notifications" element={<NotificationsPage />} />
            <Route path="a/:attachmentId/view" element={<AttachmentViewPage />} />
            <Route path=":ref/tasks" element={<ProjectTasksPage />} />
            <Route path=":ref" element={<WorkspaceRefPage />} />
          </Route>
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </BrowserRouter>
    </QueryClientProvider>
  );
}
