import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import ts from "typescript";

// Execute the maintained adapter with its actual route branches. Transport is
// captured at the existing typed client's boundary, not replaced in the app.
const text = readFileSync(new URL("./versioned-body-api.ts", import.meta.url), "utf8")
  .replace(/^import[^\n]+\n/gm, "")
  .replace(/export /g, "");
const script = ts.transpile(text, { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None });
test("project and wiki native body requests bind exact route scope, command and abort signal", async () => {
  const calls: { method: string; path: string; options: any }[] = [];
  const capture = (method: string) => async (path: string, options: any) => {
    calls.push({ method, path, options });
    return { data: { marker: calls.length } };
  };
  const adapter = runInNewContext(`${script}\n({ readVersionedBody, saveVersionedBody })`, {
    api: { GET: capture("GET"), PUT: capture("PUT") },
    ensureOk: (response: any) => response.data,
  });
  const signal = new AbortController().signal;
  const command = {
    commandId: "stable",
    expectedTailSeq: "9007199254740993",
    updateV1: "unchanged",
  };
  await adapter.readVersionedBody("ws", "doc", signal, "project-A");
  await adapter.saveVersionedBody("ws", "doc", command, "project-A");
  await adapter.readVersionedBody("ws", "doc", signal);
  await adapter.saveVersionedBody("ws", "doc", command);
  expect(calls.map((call) => call.method)).toEqual(["GET", "PUT", "GET", "PUT"]);
  for (const call of calls.slice(0, 2)) {
    expect(call.path).toBe(
      "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/body/versioned",
    );
    expect(call.options.params.path).toEqual({
      workspace_id: "ws",
      project_id: "project-A",
      document_id: "doc",
    });
  }
  for (const call of calls.slice(2)) {
    expect(call.path).toBe(
      "/api/v1/workspaces/{workspace_id}/documents/{document_id}/body/versioned",
    );
    expect(call.options.params.path).toEqual({ workspace_id: "ws", document_id: "doc" });
  }
  expect(calls[0]!.options.signal).toBe(signal);
  expect(calls[2]!.options.signal).toBe(signal);
  expect(calls[1]!.options.body).toBe(command);
  expect(calls[3]!.options.body).toBe(command);
});

test("task scope wins over project metadata and retains its exact native command", async () => {
  const calls: { path: string; options: any }[] = [];
  const capture = async (path: string, options: any) => {
    calls.push({ path, options });
    return { data: {} };
  };
  const adapter = runInNewContext(`${script}\n({ readVersionedBody, saveVersionedBody })`, {
    api: { GET: capture, PUT: capture },
    ensureOk: (reply: any) => reply.data,
  });
  const signal = new AbortController().signal;
  const command = { commandId: "stable-task", expectedTailSeq: "7", updateV1: "exact-task-bytes" };
  await adapter.readVersionedBody("ws", "task", signal, "project", "task");
  await adapter.saveVersionedBody("ws", "task", command, "project", "task");
  for (const call of calls) {
    expect(call.path).toBe("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/body/versioned");
    expect(call.options.params.path).toEqual({ workspace_id: "ws", task_id: "task" });
  }
  expect(calls[0]!.options.signal).toBe(signal);
  expect(calls[1]!.options.body).toBe(command);
});
