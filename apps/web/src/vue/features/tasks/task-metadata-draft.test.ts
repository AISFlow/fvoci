import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { compileScript, parse } from "@vue/compiler-sfc";
import * as Vue from "vue";
import type { TaskDetail } from "@/features/tasks/queries";
import * as payload from "@/features/tasks/task-edit-payload";
import * as types from "@/features/tasks/task-types";
import { compiledComponent, evaluate } from "../../../features/settings/compiled-component-test";

// Mount the actual compiled form setup and expose contract with Vue lifecycle
// and prop/watch scheduling. The template/DOM and Rust are separate browser scope.
function form() {
  let exposed: Record<string, unknown> = {};
  const filename = new URL("TaskDetailForm.vue", import.meta.url).pathname;
  const { descriptor } = parse(readFileSync(filename, "utf8"), { filename });
  const script = compileScript(descriptor, { id: "hierarchy-draft" });
  const component = compiledComponent(
    evaluate(script.content, {
      vue: Vue,
      "@fvoci/i18n": { t: (key: string) => key, formatPersonName: () => "" },
      "@nuxt/ui/components/Button.vue": {},
      "@/features/tasks/task-edit-payload": payload,
      "@/features/tasks/task-types": types,
      "@/lib/href": { formatDisplayId: () => "", itemPath: () => "" },
      "../../components/AppLink.vue": {},
      "./TaskParentSelect.vue": {},
      "@/features/projects/projects.css": {},
    }).default,
  );
  const task: Pick<
    TaskDetail,
    | "id"
    | "type"
    | "parentId"
    | "title"
    | "assigneeIds"
    | "labelIds"
    | "startDate"
    | "dueDate"
    | "dueAt"
  > = {
    id: "task-A",
    type: "task",
    parentId: "epic-A",
    title: "Original",
    assigneeIds: [],
    labelIds: [],
    startDate: null,
    dueDate: "2027-03-13",
    dueAt: null,
  };
  const props = Vue.reactive({
    slug: "acme",
    projectId: "project-A",
    projectKey: "TASK",
    statuses: [],
    members: [],
    labels: [],
    milestones: [],
    dependencyCandidates: [],
    onAddDependency: () => {},
    workspaceId: "workspace-A",
    currentUserId: "actor-A",
    readOnly: false,
    canEdit: true,
    pending: false,
    archivePending: false,
    trashPending: false,
    task,
  });
  const events: { name: string; args: unknown[] }[] = [];
  const setup = component.setup;
  let setupState: Record<string, unknown> | undefined;
  component.setup = (mountedProps, context) => {
    setupState = setup(mountedProps, {
      ...context,
      expose(value: Record<string, unknown> = {}) {
        exposed = value;
        context.expose(value);
      },
    });
    return setupState;
  };
  component.render = () => null;
  const renderer = Vue.createRenderer<object, object>({
    patchProp() {},
    insert() {},
    remove() {},
    createElement: () => ({}),
    createText: () => ({}),
    createComment: () => ({}),
    setText() {},
    setElementText() {},
    parentNode: () => null,
    nextSibling: () => null,
  });
  const eventProps = Object.fromEntries(
    ["titleBlur", "hierarchySave", "dueDateBlur", "assigneesChange", "labelsChange"].map((name) => [
      `on${name.charAt(0).toUpperCase()}${name.slice(1)}`,
      (...args: unknown[]) => events.push({ name, args }),
    ]),
  );
  const app = renderer.createApp({
    setup: () => () => Vue.h(component as Vue.Component, { ...props, ...eventProps }),
  });
  app.mount({});
  assert.ok(setupState);
  const state = setupState;
  function value(name: string): unknown {
    const field = state[name];
    assert.ok(Vue.isRef(field));
    return field.value;
  }
  function set(name: string, next: unknown) {
    const field = state[name];
    assert.ok(Vue.isRef(field));
    field.value = next;
  }
  function call(name: string, ...args: unknown[]) {
    const handler = state[name];
    assert.equal(typeof handler, "function");
    return Reflect.apply(handler as (...args: unknown[]) => unknown, undefined, args);
  }
  function type(next: string) {
    call("onTypeChange", { target: { value: next } });
  }
  return {
    props,
    events,
    draft: () => {
      const getter = exposed.getMetadataDraftState;
      assert.equal(typeof getter, "function");
      return Reflect.apply(getter as () => Record<string, unknown>, undefined, []);
    },
    value,
    set,
    call,
    type,
    stop: () => {
      app.unmount();
    },
  };
}

await test("readonly getter reports actual fields without emitting or resetting drafts", async () => {
  const f = form();
  try {
    assert.equal(f.draft().hasUnsavedMetadata, false);
    f.set("titleDraft", "미저장 제목");
    f.set("dueDateDraft", "2027-03-20");
    f.type("epic");
    const before = f.draft();
    assert.equal(before.hasUnsavedMetadata, true);
    assert.equal(before.workspaceId, "workspace-A");
    assert.equal(before.taskId, "task-A");
    assert.equal(before.actorId, "actor-A");
    assert.deepEqual(f.events, []);
    f.props.task = { ...f.props.task, title: "원격 새 제목", dueDate: "2027-03-17" };
    await Vue.nextTick();
    assert.equal(f.value("titleDraft"), "미저장 제목");
    assert.equal(f.value("dueDateDraft"), "2027-03-20");
    assert.equal(f.value("draftType"), "epic");
    assert.deepEqual(f.draft().expectedDates, {
      startDate: null,
      dueDate: "2027-03-13",
      dueAt: null,
    });
    f.call("onDueDateBlur");
    assert.deepEqual(f.events, [
      {
        name: "dueDateBlur",
        args: ["2027-03-20", { startDate: null, dueDate: "2027-03-13", dueAt: null }],
      },
    ]);
    assert.equal(before.hasUnsavedMetadata, true, "returned snapshot is not a live setter");
  } finally {
    f.stop();
  }
});

await test("untouched refill and pending actor target state are accurately reported", async () => {
  const f = form();
  try {
    f.props.task = {
      ...f.props.task,
      title: "새 제목",
      dueDate: "2027-03-17",
      type: "story",
      parentId: "epic-B",
    };
    await Vue.nextTick();
    assert.equal(f.draft().hasUnsavedMetadata, false);
    f.props.pending = true;
    await Vue.nextTick();
    assert.equal(f.draft().pending, true);
    f.props.currentUserId = "actor-B";
    f.props.workspaceId = "workspace-B";
    f.props.task = { ...f.props.task, id: "task-B" };
    await Vue.nextTick();
    assert.equal(f.draft().actorId, "actor-B");
    assert.equal(f.draft().workspaceId, "workspace-B");
    assert.equal(f.draft().taskId, "task-B");
    assert.equal(f.draft().pending, true);
    f.props.pending = false;
    await Vue.nextTick();
    assert.equal(f.draft().pending, false);
    assert.deepEqual(f.events, []);
  } finally {
    f.stop();
  }
});

await test("actual immediate assignee and label drafts are dirty until acknowledgement", async () => {
  const f = form();
  try {
    f.call("toggleAssignee", "actor-A", false);
    f.call("toggleLabel", "label-A", false);
    assert.equal(f.draft().hasUnsavedMetadata, true);
    f.props.pending = true;
    await Vue.nextTick();
    assert.equal(f.draft().pending, true);
    f.props.task = { ...f.props.task, assigneeIds: ["actor-A"], labelIds: ["label-A"] };
    await Vue.nextTick();
    assert.equal(f.draft().hasUnsavedMetadata, false);
    assert.equal(f.draft().pending, true);
    f.props.pending = false;
    await Vue.nextTick();
    assert.equal(f.draft().pending, false);
  } finally {
    f.stop();
  }
});

await test("equivalent ID ordering is untouched and returned snapshots cannot reset draft ownership", async () => {
  const f = form();
  try {
    f.props.task = {
      ...f.props.task,
      assigneeIds: ["actor-A", "actor-B"],
      labelIds: ["label-A", "label-B"],
    };
    await Vue.nextTick();
    f.set("draftAssigneeIds", ["actor-B", "actor-A"]);
    f.set("draftLabelIds", ["label-B", "label-A"]);
    assert.equal(f.draft().hasUnsavedMetadata, false);
    f.set("draftAssigneeIds", ["actor-A"]);
    const snapshot = f.draft();
    assert.equal(snapshot.hasUnsavedMetadata, true);
    assert.equal(Object.isFrozen(snapshot), true);
    assert.equal(Object.isFrozen(snapshot.dirty), true);
    assert.equal(Object.isFrozen(snapshot.expectedDates), true);
    f.props.archivePending = true;
    await Vue.nextTick();
    assert.equal(f.draft().pending, true);
    f.props.archivePending = false;
    f.props.trashPending = true;
    await Vue.nextTick();
    assert.equal(f.draft().pending, true);
    assert.equal(snapshot.hasUnsavedMetadata, true);
    assert.deepEqual(f.events, []);
  } finally {
    f.stop();
  }
});
