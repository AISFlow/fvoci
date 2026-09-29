import { t } from "@fvoci/i18n";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useEffect, type ComponentType } from "react";
import {
  createBrowserRouter,
  createRoutesFromElements,
  Navigate,
  Route,
  RouterProvider,
  useLocation,
} from "react-router-dom";
import { isVueAppPath } from "@/app-boundary";
import { SetupGuard } from "@/components/setup-guard";
import { HomePage } from "@/pages/HomePage";
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
import { TemplatesSettingsPage } from "@/pages/TemplatesSettingsPage";
import { ProjectCollectionPage } from "@/pages/ProjectCollectionPage";
import { MyTasksPage } from "@/pages/MyTasksPage";
import { ProjectWorkflowPage } from "@/pages/ProjectWorkflowPage";
import { ProjectFieldsPage } from "@/pages/ProjectFieldsPage";
import { NotificationsPage } from "@/pages/NotificationsPage";
import { ResetPasswordPage } from "@/pages/ResetPasswordPage";
import { PublicSharePage } from "@/pages/PublicSharePage";
import { WorkspaceHomePage } from "@/pages/WorkspaceHomePage";
import { MagicLinkPage } from "@/pages/MagicLinkPage";
import { ConfirmEmailPage } from "@/pages/ConfirmEmailPage";
import { CancelWithdrawPage } from "@/pages/CancelWithdrawPage";
import { AccountSettingsPage } from "@/pages/AccountSettingsPage";
import { ConsentPage } from "@/pages/ConsentPage";
import { ServiceInfoPage } from "@/pages/ServiceInfoPage";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      refetchOnWindowFocus: false,
    },
  },
});

/**
 * Paths no React route renders. A Vue app path (src/app-boundary.ts) reached
 * by an in-app navigation is loaded as a new page, so the boot module starts
 * the Vue app for it; anything else goes home as before.
 */
function NoRoute() {
  const { pathname, search, hash } = useLocation();
  const vuePage = isVueAppPath(pathname);
  useEffect(() => {
    if (vuePage) window.location.assign(`${pathname}${search}${hash}`);
  }, [vuePage, pathname, search, hash]);
  return vuePage ? null : <Navigate to="/" replace />;
}

function RouteLoading() {
  return <p role="status">{t("load.loading")}</p>;
}

/**
 * A page kept out of the main bundle (admin, attachment viewers, legal). The router loads it before rendering the route: on a navigation
 * the current page stays until it is in, and on a page load only this route
 * shows `RouteLoading` while its parents render and fetch as usual. Not a
 * React.lazy Suspense boundary: React holds a boundary's reveal until 300 ms
 * after its fallback appeared.
 */
function lazyPage(load: () => Promise<ComponentType>, options: { setupGuard?: boolean } = {}) {
  return {
    HydrateFallback: RouteLoading,
    lazy: async () => {
      const Page = await load();
      return options.setupGuard
        ? {
            element: (
              <SetupGuard>
                <Page />
              </SetupGuard>
            ),
          }
        : { Component: Page };
    },
  };
}

// A data router, so pages can hold navigation behind unsaved edits (`useBlocker`).
// Built once per page load, outside React, so StrictMode does not start a second one.
const router = createBrowserRouter(
  createRoutesFromElements(
    <>
      <Route path="/setup" element={<SetupPage />} />
      {/* Public share reader: no session and no setup guard (it must not redirect to /login). */}
      <Route path="/s/:token" element={<PublicSharePage />} />
      <Route
        path="/s/:token/attachments/:attachmentId/view"
        {...lazyPage(() => import("@/pages/ShareAttachmentViewPage").then((m) => m.ShareAttachmentViewPage))}
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
      <Route path="/service-info" element={<ServiceInfoPage />} />
      <Route path="/legal/:kind" {...lazyPage(() => import("@/pages/LegalPage").then((m) => m.LegalPage))} />
      <Route
        path="/settings/admin"
        {...lazyPage(() => import("@/pages/AdminPage").then((m) => m.AdminPage), { setupGuard: true })}
      />
      <Route
        path="/settings/audit"
        {...lazyPage(() => import("@/pages/AdminAuditPage").then((m) => m.AdminAuditPage), { setupGuard: true })}
      />
      <Route
        path="/settings/legal"
        {...lazyPage(() => import("@/pages/AdminLegalPage").then((m) => m.AdminLegalPage), { setupGuard: true })}
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
        <Route path="my-tasks" element={<MyTasksPage />} />
        <Route path="wiki" element={<WikiPage />} />
        <Route path="search" element={<SearchPage />} />
        <Route path="trash" element={<TrashPage />} />
        <Route path="settings" element={<WorkspaceSettingsPage />} />
        <Route path="settings/document-tags" element={<DocumentTagsSettingsPage />} />
        <Route path="settings/templates" element={<TemplatesSettingsPage />} />
        <Route path="notifications" element={<NotificationsPage />} />
        <Route
          path="a/:attachmentId/view"
          {...lazyPage(() => import("@/pages/AttachmentViewPage").then((m) => m.AttachmentViewPage))}
        />
        <Route path=":ref/tasks" element={<ProjectTasksPage />} />
        <Route path=":ref/table" element={<ProjectCollectionPage type="table" />} />
        <Route path=":ref/board" element={<ProjectCollectionPage type="board" />} />
        <Route path=":ref/calendar" element={<ProjectCollectionPage type="calendar" />} />
        <Route path=":ref/settings/fields" element={<ProjectFieldsPage />} />
        <Route path=":ref/settings/workflow" element={<ProjectWorkflowPage />} />
        <Route path=":ref" element={<WorkspaceRefPage />} />
      </Route>
      <Route path="*" element={<NoRoute />} />
    </>,
  ),
);

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
}
