import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useEffect } from "react";
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
import { WorkspaceLayout } from "@/pages/WorkspaceLayout";
import { WorkspaceRefPage } from "@/pages/WorkspaceRefPage";

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

// A data router, so pages can hold navigation behind unsaved edits (`useBlocker`).
// Built once per page load, outside React, so StrictMode does not start a second one.
const router = createBrowserRouter(
  createRoutesFromElements(
    <>
      {/* Public share reader: no session and no setup guard (it must not redirect to /login). */}
      <Route path="/s/:token" element={<NoRoute />} />
      <Route path="/settings/admin" element={<NoRoute />} />
      <Route path="/settings/audit" element={<NoRoute />} />
      <Route path="/settings/legal" element={<NoRoute />} />
      <Route path="/settings/account" element={<NoRoute />} />
      <Route
        path="/w/:slug"
        element={
          <SetupGuard>
            <WorkspaceLayout />
          </SetupGuard>
        }
      >
        <Route index element={<NoRoute />} />
        <Route path="projects" element={<NoRoute />} />
        <Route path="my-tasks" element={<NoRoute />} />
        <Route path="wiki" element={<NoRoute />} />
        <Route path="search" element={<NoRoute />} />
        <Route path="trash" element={<NoRoute />} />
        <Route path="settings" element={<NoRoute />} />
        <Route path="settings/document-tags" element={<NoRoute />} />
        <Route path="settings/templates" element={<NoRoute />} />
        <Route path="notifications" element={<NoRoute />} />
        <Route path=":ref/settings/fields" element={<NoRoute />} />
        <Route path=":ref/settings/workflow" element={<NoRoute />} />
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
