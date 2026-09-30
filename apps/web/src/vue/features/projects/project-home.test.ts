import assert from "node:assert/strict";
import test from "node:test";
import { QueryClient, useQuery, VueQueryPlugin } from "@tanstack/vue-query";
import { createApp, effectScope } from "vue";
import { projectDocumentsQuery, projectQuery } from "@/features/projects/queries";
import { isVueAppPath } from "@/app-boundary";
import { PROJECT_HOME_PATH } from "@/app-boundary";
import { VUE_ROUTE_PATHS } from "../../route-paths.ts";
import { leaveTo } from "../../session/navigation.ts";
import { projectHomeChildNodes } from "./project-home.ts";

function queryClient(): QueryClient {
  return new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
}

function mount<T>(client: QueryClient, use: () => T): { result: T; stop: () => void } {
  const app = createApp({ render: () => null });
  app.use(VueQueryPlugin, { queryClient: client });
  const scope = effectScope();
  const result = app.runWithContext(() => scope.run(use)) as T;
  return {
    result,
    stop: () => {
      scope.stop();
      client.clear();
    },
  };
}

test("the live project-home path is /w/:slug/:ref like the collection routes", () => {
  assert.ok(VUE_ROUTE_PATHS.projectHome.startsWith("/w/:slug/:ref("));
});

test("the live project-home boundary regex takes project keys only", () => {
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/GNT"), true);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/gnt"), true);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/GNT/"), true);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/wiki-3"), false);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/WIKI-3"), false);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/GNT-1"), false);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/GNT/gantt"), false);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/GNT/tasks"), false);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/GNT/board"), false);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/GNT/calendar"), false);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/GNT/table"), false);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/GNT/settings"), false);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/projects"), false);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/search"), false);
  assert.equal(PROJECT_HOME_PATH.test("/w/acme/wiki"), false);
  assert.equal(isVueAppPath("/w/acme/GNT"), true, "boundary now renders Vue");
  assert.equal(isVueAppPath("/w/acme/wiki-3"), true);
  assert.equal(isVueAppPath("/w/acme/GNT/gantt"), true);
});

test("project detail and documents queries wait for workspace and project ids", () => {
  assert.equal(projectQuery("", "p").enabled, false);
  assert.equal(projectQuery("w", "").enabled, false);
  assert.equal(projectQuery("w", "p").enabled, true);
  assert.equal(projectDocumentsQuery("", "p").enabled, false);
  assert.equal(projectDocumentsQuery("w", "").enabled, false);
  assert.equal(projectDocumentsQuery("w", "p").enabled, true);
});

test("project home queries stay idle until both ids exist", () => {
  const client = queryClient();
  const { result, stop } = mount(client, () => ({
    project: useQuery(() => projectQuery("", "")),
    documents: useQuery(() => projectDocumentsQuery("", "")),
  }));
  try {
    assert.equal(result.project.fetchStatus.value, "idle");
    assert.equal(result.documents.fetchStatus.value, "idle");
    assert.equal(result.project.isFetching.value, false);
    assert.equal(result.documents.isFetching.value, false);
  } finally {
    stop();
  }
});

test("after delete the live projects list stays Vue and private settings full-load React", () => {
  const assigns: string[] = [];
  const pushes: string[] = [];
  const env = {
    assign: (url: string) => assigns.push(url),
    push: (path: string) => pushes.push(path),
  };
  leaveTo("/w/acme/projects", env);
  assert.deepEqual(assigns, []);
  assert.deepEqual(pushes, ["/w/acme/projects"]);
  leaveTo("/w/acme/GNT/gantt", env);
  assert.deepEqual(assigns, []);
  assert.deepEqual(pushes, ["/w/acme/projects", "/w/acme/GNT/gantt"]);
});

test("project home lists the project's root children, nested ones stay in the tree", () => {
  const project = {
    id: "p1",
    rootDocumentId: "root",
  } as const;
  const nodes = [
    {
      id: "root",
      parentId: null,
      projectId: "p1",
      number: 1,
      title: "root",
      icon: null,
      path: "a",
      sortKey: "a",
      status: "published",
      workspaceId: "w",
    },
    {
      id: "child",
      parentId: "root",
      projectId: "p1",
      number: 2,
      title: "child",
      icon: "📄",
      path: "a/b",
      sortKey: "b",
      status: "published",
      workspaceId: "w",
    },
    {
      id: "nested",
      parentId: "child",
      projectId: "p1",
      number: 3,
      title: "nested",
      icon: null,
      path: "a/b/c",
      sortKey: "c",
      status: "published",
      workspaceId: "w",
    },
    {
      id: "other",
      parentId: "root",
      projectId: "p2",
      number: 9,
      title: "other",
      icon: null,
      path: "x",
      sortKey: "x",
      status: "published",
      workspaceId: "w",
    },
  ];
  assert.deepEqual(
    projectHomeChildNodes(nodes, {
      ...project,
      createdAt: "",
      createdBy: "",
      description: null,
      icon: null,
      key: "GNT",
      name: "Gantt",
      status: "active",
      updatedAt: "",
      visibility: "workspace",
      workspaceId: "w",
    }).map((node) => node.id),
    ["child"],
  );
});
