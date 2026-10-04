import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { compileScript, parse } from "@vue/compiler-sfc";
import * as Query from "@tanstack/vue-query";
import { onlineManager } from "@tanstack/query-core";
import * as Vue from "vue";
import * as i18n from "@fvoci/i18n";
import * as board from "@/features/collections/board-model";
import * as calendarModel from "@/features/collections/calendar-model";
import * as collectionView from "@/features/collections/collection-view";
import * as taskPatch from "@/features/tasks/task-patch-cache";
import * as taskPayload from "@/features/tasks/task-edit-payload";
import * as values from "@/lib/collection-values";
import * as api from "@/lib/api";
import * as datetime from "@/lib/datetime";
import * as href from "@/lib/href";
import * as queries from "@/lib/queries";
import * as collections from "@/lib/queries/collections";
import * as viewQuery from "@/lib/view-query";
import { evaluate } from "../../../features/settings/compiled-component-test";
import * as adapter from "./calendar/calendar-adapter";

// Actual compiled parent, contents, Calendar and editor, real Vue Query and
// renderer. Only transport and UI-library leaf controls/host DOM are controlled:
// these mounted tests are not browser, Rust, DB or Popover-interaction evidence.
class Host extends EventTarget {
  text = "";
  props: Record<string, unknown> = {};
  children: Host[] = [];
  parent: Host | null = null;
  value = "";
  type = "";
  constructor(readonly tag: string) {
    super();
  }
  getRootNode(): Host {
    return this.parent?.getRootNode() ?? this;
  }
  getBoundingClientRect() {
    return { x: 0, y: 0, width: 100, height: 30 };
  }
}
const renderer = Vue.createRenderer<Host, Host>({
  createElement: (tag) => new Host(tag),
  createText: (text) => Object.assign(new Host("text"), { text }),
  createComment: (text) => Object.assign(new Host("comment"), { text }),
  setText: (el, text) => {
    el.text = text;
  },
  setElementText: (el, text) => {
    el.text = text;
    el.children = [];
  },
  patchProp: (el, key, _old, value) => {
    el.props[key] = value;
    if (key === "value" || key === "type") el[key] = String(value ?? "");
  },
  parentNode: (el) => el.parent,
  nextSibling: (el) => el.parent?.children[el.parent.children.indexOf(el) + 1] ?? null,
  insert: (el, parent, anchor) => {
    if (el.parent) el.parent.children.splice(el.parent.children.indexOf(el), 1);
    const index = anchor ? parent.children.indexOf(anchor) : -1;
    parent.children.splice(index < 0 ? parent.children.length : index, 0, el);
    el.parent = parent;
  },
  remove: (el) => {
    el.parent?.children.splice(el.parent.children.indexOf(el), 1);
    el.parent = null;
  },
});
function descendants(el: Host): Host[] {
  return [el, ...el.children.flatMap(descendants)];
}
function compile(path: string, imports: Record<string, unknown>): Vue.Component {
  const filename = new URL(path, import.meta.url).pathname;
  const { descriptor } = parse(readFileSync(filename, "utf8"), { filename });
  const script = compileScript(descriptor, { id: path, inlineTemplate: true });
  return evaluate(script.content, imports).default as Vue.Component;
}
const button = Vue.defineComponent({
  setup:
    (_props, { attrs, slots }) =>
    () =>
      Vue.h("button", attrs, slots.default?.()),
});
const blank = Vue.defineComponent({ setup: () => () => null });
const popover = Vue.defineComponent({
  props: ["open"],
  setup:
    (props, { slots }) =>
    () =>
      Vue.h("popover", { "data-state": props.open ? "open" : "closed" }, [
        slots.default?.(),
        props.open ? slots.content?.() : null,
      ]),
});
const common = {
  vue: Vue,
  "@fvoci/i18n": i18n,
  "@nuxt/ui/components/Button.vue": { default: button },
  "@/features/collections/calendar-model": calendarModel,
  "@/lib/href": href,
};
const errorComponent = compile("../../components/QueryError.vue", common);
const eventEditor = compile("./calendar/CalendarEventEditor.vue", {
  ...common,
  "./calendar-adapter": adapter,
});
const calendar = compile("./calendar/CollectionCalendar.vue", {
  ...common,
  "@nuxt/ui/components/Tabs.vue": { default: blank },
  "@nuxt/ui/components/Popover.vue": { default: popover },
  "@/features/tasks/task-edit-payload": taskPayload,
  "@/lib/collection-values": values,
  "@/features/collections/collection-view": collectionView,
  "./calendar-adapter": adapter,
  "./CalendarMini.vue": { default: blank },
  "./CalendarEventEditor.vue": { default: eventEditor },
  "./calendar.css": {},
});
function deferred() {
  let reject!: (error: unknown) => void;
  let resolve!: (value: unknown) => void;
  const promise = new Promise<unknown>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, reject, resolve };
}
async function until(check: () => boolean, message: string) {
  for (let i = 0; i < 200; i++) {
    await Vue.nextTick();
    if (check()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.fail(`timed out waiting for ${message}`);
}
function click(el: Host) {
  assert.equal(typeof el.props.onClick, "function");
  return Reflect.apply(el.props.onClick as (...args: unknown[]) => unknown, undefined, [
    { currentTarget: el, stopPropagation() {}, preventDefault() {} },
  ]);
}

async function mountCalendar(initialFailure?: { kind: string; error: Error }) {
  const savedOnline = onlineManager.isOnline();
  onlineManager.setOnline(true);
  const savedWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const savedNavigator = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  Object.defineProperty(globalThis, "window", { configurable: true, value: new EventTarget() });
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: { onLine: true } });
  const props = Vue.reactive({
    slug: "workspace",
    workspace: { id: "workspace-A" },
    project: { id: "project-A", key: "P" },
    type: "calendar",
  });
  const client = new Query.QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: Infinity } },
  });
  client.mount();
  const requests: { kind: string; target: string }[] = [];
  const pending = new Map<string, ReturnType<typeof deferred>>();
  let writes = 0;
  const controlledApi = {
    ...api,
    api: {
      ...api.api,
      PATCH: () => {
        writes++;
        return Promise.reject(new Error("unexpected write from the mounted offline Calendar"));
      },
    },
  };
  const row = {
    id: "document-A",
    taskId: "task-A",
    displayId: "P-1",
    title: "Calendar task",
    date: values.todayInTimeZone("UTC"),
    dueAt: new Date().toISOString(),
    dueDate: null,
    startDate: null,
    canEdit: true,
    values: {},
  };
  let actor = "actor-A";
  const responses = (kind: string, target: string): unknown => {
    if (kind === "collection") return { id: `collection-${target}` };
    if (kind === "fields" || kind === "members") return { items: [] };
    if (kind === "views") return { items: [], canSave: true, canManage: true };
    if (kind === "me") return { userId: actor, timezone: "UTC", weekStartsOn: 1 };
    return { items: [], count: 1, previews: [row], days: [], canEdit: true };
  };
  const request = (kind: string, target = "") => {
    requests.push({ kind, target });
    if (initialFailure?.kind === kind) return Promise.reject(initialFailure.error);
    const held = pending.get(kind);
    if (held) {
      pending.delete(kind);
      return held.promise;
    }
    return Promise.resolve(responses(kind, target));
  };
  const controlledQueries = {
    ...collections,
    projectCollectionQuery: (ws: string, project: string) => ({
      ...collections.projectCollectionQuery(ws, project),
      queryFn: () => request("collection", project),
    }),
    collectionFieldsQuery: (ws: string, id: string) => ({
      ...collections.collectionFieldsQuery(ws, id),
      queryFn: () => request("fields", id),
    }),
    collectionViewsQuery: (ws: string, id: string) => ({
      ...collections.collectionViewsQuery(ws, id),
      queryFn: () => request("views", id),
    }),
    collectionRowsQuery: (...args: Parameters<typeof collections.collectionRowsQuery>) => ({
      ...collections.collectionRowsQuery(...args),
      queryFn: () => request("rows", args[1]),
    }),
  };
  const contents = compile("./CollectionContents.vue", {
    ...common,
    "@tanstack/vue-query": Query,
    "@/features/collections/board-model": board,
    "@/features/collections/collection-view": collectionView,
    "@/features/tasks/task-patch-cache": taskPatch,
    "@/lib/collection-values": values,
    "@/lib/datetime": datetime,
    "@/lib/api": controlledApi,
    "@/lib/queries": {
      ...queries,
      meQuery: { ...queries.meQuery, queryFn: () => request("me") },
      membersQuery: (ws: string) => ({
        ...queries.membersQuery(ws),
        queryFn: () => request("members"),
      }),
    },
    "@/lib/queries/collections": controlledQueries,
    "@/lib/view-query": viewQuery,
    "../../components/ConfirmActionButton.vue": { default: blank },
    "../../components/QueryError.vue": { default: errorComponent },
    "../../components/QueryLoading.vue": { default: blank },
    "./CollectionBoard.vue": { default: blank },
    "./calendar/CollectionCalendar.vue": { default: calendar },
    "./calendar/calendar-adapter": adapter,
    "./CustomFilters.vue": { default: blank },
    "./ValueEditor.vue": { default: blank },
    "@/features/collections/collections.css": {},
  });
  const panel = compile("./CollectionPanel.vue", {
    ...common,
    "@tanstack/vue-query": Query,
    "vue-router": {
      useRoute: () => ({ query: {} }),
      useRouter: () => ({ push() {}, replace() {} }),
    },
    "@/lib/api": api,
    "@/lib/queries/collections": controlledQueries,
    "../../components/QueryError.vue": { default: errorComponent },
    "./CollectionContents.vue": { default: contents },
  });
  const root = new Host("root");
  const app = renderer.createApp({ setup: () => () => Vue.h(panel, props) });
  app.use(Query.VueQueryPlugin, { queryClient: client });
  app.mount(root);
  await Vue.nextTick();
  const key = (kind: string) =>
    kind === "collection"
      ? collections.projectCollectionQuery(props.workspace.id, props.project.id).queryKey
      : kind === "me"
        ? queries.meQuery.queryKey
        : [
            ...collections.collectionPrefix(props.workspace.id, `collection-${props.project.id}`),
            kind,
          ];
  return {
    props,
    client,
    requests,
    root,
    writes: () => writes,
    input: () =>
      descendants(root).find((el) => el.tag === "input" && el.props.type === "datetime-local"),
    errors: () => descendants(root).filter((el) => el.props.role === "alert"),
    async open() {
      await until(
        () => descendants(root).some((el) => el.props["data-event"] !== undefined),
        "actual Calendar chip",
      );
      click(required(descendants(root).find((el) => el.props["data-event"] !== undefined)));
      await until(() => Boolean(this.input()), "actual Calendar editor");
      const input = required(this.input());
      input.value = "2026-11-02T09:30";
      input.dispatchEvent(new Event("input"));
      await Vue.nextTick();
      return input;
    },
    async fail(kind: string, error: unknown, offline = false) {
      const held = deferred();
      pending.set(kind, held);
      const loading = client.refetchQueries({ queryKey: key(kind), exact: true });
      await until(
        () => client.getQueryState(key(kind))?.fetchStatus === "fetching",
        "background request",
      );
      assert.notEqual(
        client.getQueryData(key(kind)),
        undefined,
        "genuine prior successful query data",
      );
      // The actual CI request started online before the browser went offline.
      if (offline) window.dispatchEvent(new Event("offline"));
      held.reject(error);
      await loading;
      await until(
        () => client.getQueryState(key(kind))?.status === "error",
        "actual query error state",
      );
      await Vue.nextTick();
      console.log("metadata failure", {
        kind,
        dataPresent: client.getQueryData(key(kind)) !== undefined,
        error: error instanceof Error ? error.constructor.name : typeof error,
        editorPresent: Boolean(this.input()),
      });
    },
    async retry() {
      const before = requests.length;
      const alert = required(this.errors()[0]);
      await click(required(descendants(alert).find((el) => el.tag === "button")));
      await until(() => this.errors().length === 0, "successful owned retry");
      return requests.slice(before).map((r) => r.kind);
    },
    async actor(next: string) {
      actor = next;
      await client.refetchQueries({ queryKey: key("me"), exact: true });
      await Vue.nextTick();
    },
    stop() {
      app.unmount();
      client.unmount();
      client.clear();
      onlineManager.setOnline(savedOnline);
      if (savedWindow) Object.defineProperty(globalThis, "window", savedWindow);
      else Reflect.deleteProperty(globalThis, "window");
      if (savedNavigator) Object.defineProperty(globalThis, "navigator", savedNavigator);
      else Reflect.deleteProperty(globalThis, "navigator");
    },
  };
}
function required<T>(value: T | undefined): T {
  assert.notEqual(value, undefined);
  return value as T;
}

for (const kind of ["fields", "collection", "views"])
  await test(`cached ${kind} transport failure retains the actual Calendar editor and owned retry`, async () => {
    const h = await mountCalendar();
    try {
      const input = await h.open();
      await h.fail(kind, new TypeError("Failed to fetch"));
      assert.equal(h.input(), input, `${kind}: the same Calendar input must stay mounted`);
      assert.equal(input.value, "2026-11-02T09:30");
      assert.equal(h.errors().length, 1);
      assert.deepEqual(await h.retry(), [kind], "Retry refetches its failed metadata owner");
      assert.equal(h.input(), input);
      assert.equal(input.value, "2026-11-02T09:30");
    } finally {
      h.stop();
    }
  });

for (const kind of ["collection", "views"])
  for (const status of [401, 403, 404])
    await test(`cached ${kind} HTTP${String(status)} retires the editor`, async () => {
      const h = await mountCalendar();
      try {
        await h.open();
        await h.fail(kind, new api.ProblemError(status));
        assert.equal(h.input(), undefined);
      } finally {
        h.stop();
      }
    });

for (const kind of ["collection", "views"])
  await test(`initial ${kind} transport error without cached data stays fatal`, async () => {
    const h = await mountCalendar({ kind, error: new TypeError("Failed to fetch") });
    try {
      await until(() => h.errors().length === 1, "initial error gate");
      assert.equal(h.input(), undefined);
    } finally {
      h.stop();
    }
  });

await test("project and actor changes retire the previous Calendar draft", async () => {
  const h = await mountCalendar();
  try {
    const first = await h.open();
    h.props.project = { id: "project-B", key: "B" };
    await Vue.nextTick();
    assert.equal(h.input(), undefined);
    const second = await h.open();
    assert.notEqual(second, first);
    await h.actor("actor-B");
    assert.equal(h.input(), undefined);
    await h.actor("actor-A");
    assert.equal(h.input(), undefined, "returning to the prior actor cannot resurrect its draft");
    h.props.project = { id: "project-A", key: "P" };
    await Vue.nextTick();
    assert.equal(h.input(), undefined, "returning to the prior target cannot resurrect its draft");
  } finally {
    h.stop();
  }
});

for (const kind of ["collection", "views"])
  await test(`cached ${kind} typed503 retains the editor but501 and unexpected errors do not`, async () => {
    const h = await mountCalendar();
    try {
      const input = await h.open();
      await h.fail(kind, new api.ProblemError(503));
      assert.equal(h.input(), input);
      await h.retry();
      await h.fail(kind, new api.ProblemError(501));
      assert.equal(h.input(), undefined);
      await h.retry();
      await h.open();
      await h.fail(kind, new SyntaxError("unexpected metadata decoder failure"));
      assert.equal(h.input(), undefined);
    } finally {
      h.stop();
    }
  });

await test("cached me denial remains fatal even when Calendar metadata is retained", async () => {
  const h = await mountCalendar();
  try {
    await h.open();
    await h.fail("me", new api.ProblemError(401));
    assert.equal(h.input(), undefined);
  } finally {
    h.stop();
  }
});

for (const kind of ["collection", "views"])
  await test(`cached ${kind} failure offline preserves draft but refuses submit`, async () => {
    const h = await mountCalendar();
    try {
      const input = await h.open();
      await h.fail(kind, new TypeError("Failed to fetch"), true);
      assert.equal(h.input(), input);
      const form = required(
        descendants(h.root).find((el) => el.props["aria-label"] === "Calendar event editor"),
      );
      const save = required(
        descendants(form).find((el) => el.tag === "button" && el.props.type === "submit"),
      );
      assert.equal(save.props.disabled, true);
      assert.equal(typeof form.props.onSubmit, "function");
      await Reflect.apply(form.props.onSubmit as (...args: unknown[]) => unknown, undefined, [
        { preventDefault() {} },
      ]);
      assert.equal(h.writes(), 0);
      assert.equal(input.value, "2026-11-02T09:30");
    } finally {
      h.stop();
    }
  });
