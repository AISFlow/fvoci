import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { compileScript, parse } from "@vue/compiler-sfc";
import * as Vue from "vue";
import type { TaskDetail } from "@/features/tasks/queries";
import * as payload from "@/features/tasks/task-edit-payload";
import * as types from "@/features/tasks/task-types";
import { compiledComponent, evaluate } from "../../../features/settings/compiled-component-test";

// Execute the actual form setup with real reactive props/watch scheduling.
// This is a local ownership check; the browser group supplies Rust/DB oracles.
function form() {
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
    workspaceId: "workspace-A",
    currentUserId: "actor-A",
    readOnly: false,
    canEdit: true,
    pending: false,
    task,
  });
  const events: { name: string; args: unknown[] }[] = [];
  const scope = Vue.effectScope();
  const state = scope.run(() =>
    component.setup(props, {
      attrs: {},
      slots: {},
      expose() {},
      emit: (name: string, ...args: unknown[]) => events.push({ name, args }),
    }),
  );
  assert.ok(state);
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
    value,
    set,
    call,
    type,
    stop: () => {
      scope.stop();
    },
  };
}

await test("same-task status/priority replacement preserves epic/null until Save", async () => {
  const f = form();
  try {
    f.type("epic");
    f.props.task = { ...f.props.task };
    await Vue.nextTick();
    assert.equal(f.value("draftType"), "epic");
    assert.equal(f.value("draftParentId"), null);
    assert.equal(f.value("hierarchyDirty"), true);
    assert.deepEqual(f.events, []);
    f.call("saveHierarchy");
    assert.deepEqual(f.events, [{ name: "hierarchySave", args: ["epic", null] }]);
    f.props.task = { ...f.props.task, type: "epic", parentId: null };
    await Vue.nextTick();
    assert.equal(f.value("hierarchyDirty"), false);
  } finally {
    f.stop();
  }
});

await test("dirty parent survives refetch while untouched type follows committed values", async () => {
  const f = form();
  try {
    f.set("draftParentId", "epic-B");
    f.props.task = { ...f.props.task, type: "story", parentId: "epic-C" };
    await Vue.nextTick();
    assert.equal(f.value("draftType"), "story");
    assert.equal(f.value("draftParentId"), "epic-B");
    f.call("cancelHierarchy");
    assert.equal(f.value("draftParentId"), "epic-C");
    assert.equal(f.value("hierarchyDirty"), false);
  } finally {
    f.stop();
  }
});

await test("dirty type preserves only its field; untouched parent follows live updates", async () => {
  const f = form();
  try {
    f.type("bug");
    f.props.task = { ...f.props.task, type: "story", parentId: "epic-B" };
    await Vue.nextTick();
    assert.equal(f.value("draftType"), "bug");
    assert.equal(f.value("draftParentId"), "epic-B");
    f.props.task = { ...f.props.task, type: "bug" };
    await Vue.nextTick();
    assert.equal(f.value("hierarchyDirty"), false);
    f.props.task = { ...f.props.task, type: "task", parentId: null };
    await Vue.nextTick();
    assert.equal(f.value("draftType"), "task");
    assert.equal(f.value("draftParentId"), null);
  } finally {
    f.stop();
  }
});

await test("epic and subtask boundary edits own the induced parent clear", async () => {
  for (const type of ["epic", "subtask"]) {
    const f = form();
    try {
      f.props.task.parentId = null;
      await Vue.nextTick();
      f.type(type);
      f.props.task = { ...f.props.task, parentId: "epic-B" };
      await Vue.nextTick();
      assert.equal(f.value("draftType"), type);
      assert.equal(f.value("draftParentId"), null);
    } finally {
      f.stop();
    }
  }
});

await test("target and actor ABA retire hierarchy even within one Vue tick", async () => {
  for (const boundary of ["task", "workspace", "actor"]) {
    const f = form();
    try {
      f.type("epic");
      if (boundary === "task") {
        f.props.task.id = "task-B";
        f.props.task.id = "task-A";
      }
      if (boundary === "workspace") {
        f.props.workspaceId = "workspace-B";
        f.props.workspaceId = "workspace-A";
      }
      if (boundary === "actor") {
        f.props.currentUserId = "actor-B";
        f.props.currentUserId = "actor-A";
      }
      await Vue.nextTick();
      assert.equal(f.value("draftType"), "task", boundary);
      assert.equal(f.value("draftParentId"), "epic-A", boundary);
      assert.equal(f.value("hierarchyDirty"), false, boundary);
    } finally {
      f.stop();
    }
  }
});

await test("pending/failure keeps a draft; permission retirement discards it", async () => {
  for (const boundary of ["readOnly", "canEdit"] as const) {
    const f = form();
    try {
      f.type("epic");
      f.props.pending = true;
      await Vue.nextTick();
      f.props.pending = false;
      await Vue.nextTick();
      assert.equal(f.value("draftType"), "epic");
      f.props[boundary] = boundary === "readOnly";
      f.props[boundary] = boundary !== "readOnly";
      await Vue.nextTick();
      assert.equal(f.value("draftType"), "task");
      assert.equal(f.value("hierarchyDirty"), false);
    } finally {
      f.stop();
    }
  }
});

await test("unrelated metadata refetch keeps the original due-date expectedDates snapshot", async () => {
  const f = form();
  try {
    f.type("epic");
    f.set("dueDateDraft", "2027-03-20");
    f.props.task = { ...f.props.task, dueDate: "2027-03-17" };
    await Vue.nextTick();
    f.call("onDueDateBlur");
    assert.deepEqual(f.events, [
      {
        name: "dueDateBlur",
        args: ["2027-03-20", { startDate: null, dueDate: "2027-03-13", dueAt: null }],
      },
    ]);
  } finally {
    f.stop();
  }
});
