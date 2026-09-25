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
import { DocumentTagsSettingsPage } from "@/pages/DocumentTagsSettingsPage";
import { ProjectCollectionPage } from "@/pages/ProjectCollectionPage";
import { ProjectFieldsPage } from "@/pages/ProjectFieldsPage";
import { NotificationsPage } from "@/pages/NotificationsPage";
import { InvitePage } from "@/pages/InvitePage";
import { AttachmentViewPage } from "@/pages/AttachmentViewPage";
import { ResetPasswordPage } from "@/pages/ResetPasswordPage";
import { PublicSharePage } from "@/pages/PublicSharePage";
import { WorkspaceHomePage } from "@/pages/WorkspaceHomePage";
import { MagicLinkPage } from "@/pages/MagicLinkPage";
import { ConfirmEmailPage } from "@/pages/ConfirmEmailPage";
import { CancelWithdrawPage } from "@/pages/CancelWithdrawPage";
import { AccountSettingsPage } from "@/pages/AccountSettingsPage";
import { AdminPage } from "@/pages/AdminPage";
import { AdminAuditPage } from "@/pages/AdminAuditPage";
import { AdminLegalPage } from "@/pages/AdminLegalPage";
import { ConsentPage } from "@/pages/ConsentPage";
import { LegalPage } from "@/pages/LegalPage";

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
          <Route path="/consent" element={<ConsentPage />} />
          <Route path="/legal/:kind" element={<LegalPage />} />
          <Route
            path="/settings/admin"
            element={
              <SetupGuard>
                <AdminPage />
              </SetupGuard>
            }
          />
          <Route
            path="/settings/audit"
            element={
              <SetupGuard>
                <AdminAuditPage />
              </SetupGuard>
            }
          />
          <Route
            path="/settings/legal"
            element={
              <SetupGuard>
                <AdminLegalPage />
              </SetupGuard>
            }
          />
          <Route
            path="/magic-link"
            element={
              <SetupGuard>
                <MagicLinkPage />
              </SetupGuard>
            }
          />
          <Route
            path="/confirm-email"
            element={
              <SetupGuard>
                <ConfirmEmailPage />
              </SetupGuard>
            }
          />
          <Route
            path="/cancel-withdraw"
            element={
              <SetupGuard>
                <CancelWithdrawPage />
              </SetupGuard>
            }
          />
          <Route
            path="/settings/account"
            element={
              <SetupGuard>
                <AccountSettingsPage />
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
            <Route path="settings/document-tags" element={<DocumentTagsSettingsPage />} />
            <Route path="notifications" element={<NotificationsPage />} />
            <Route path="a/:attachmentId/view" element={<AttachmentViewPage />} />
            <Route path=":ref/tasks" element={<ProjectTasksPage />} />
            <Route path=":ref/table" element={<ProjectCollectionPage type="table" />} />
            <Route path=":ref/board" element={<ProjectCollectionPage type="board" />} />
            <Route path=":ref/calendar" element={<ProjectCollectionPage type="calendar" />} />
            <Route path=":ref/settings/fields" element={<ProjectFieldsPage />} />
            <Route path=":ref" element={<WorkspaceRefPage />} />
          </Route>
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </BrowserRouter>
    </QueryClientProvider>
  );
}
