import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import type { OffDraftCreateBody } from "./document-api";
type RequestOptions = {
  params: { path: Record<string, string> };
  body: OffDraftCreateBody;
  signal?: AbortSignal;
};

const source = readFileSync(new URL("./document-api.ts", import.meta.url), "utf8")
  .replace(/^import[^\n]+\n/gm, "")
  .replace(/export /g, "");
const script = ts.transpile(source, { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None });
test("from-draft transport keeps source and destination distinct and replays exact captured body", async () => {
  const calls: { path: string; options: RequestOptions }[] = [];
  const adapter = runInNewContext(`${script}\ncreateDocumentFromDraft`, {
    api: {
      POST: (path: string, options: RequestOptions) =>
        new Promise<{ data: unknown }>((resolve) => {
          calls.push({ path, options });
          resolve({ data: { commandId: options.body.commandId } });
          return;
        }),
    },
    ensureOk: (response: { data: unknown }) => response.data,
  }) as typeof import("./document-api").createDocumentFromDraft;
  const body: OffDraftCreateBody = {
    commandId: "captured",
    sourceKind: "task",
    sourceId: "source-task",
    sourceProjectId: null,
    parentId: "destination-parent",
    title: "copy",
    contentJson: { type: "doc", content: [] },
  };
  const signal = new AbortController().signal;
  await adapter("ws", "destination-project", body, signal);
  await adapter("ws", "destination-project", body, signal);
  await adapter("ws", null, body, signal);
  expect(calls[0].path).toBe(
    "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/from-draft",
  );
  expect(calls[0].options.params.path).toEqual({
    workspace_id: "ws",
    project_id: "destination-project",
  });
  expect(calls[1].options.body).toBe(body);
  expect(calls[0].options.body).toBe(body);
  expect(calls[0].options.body.sourceId).toBe("source-task");
  expect(calls[0].options.signal).toBe(signal);
  expect(calls[2].path).toBe("/api/v1/workspaces/{workspace_id}/documents/from-draft");
  expect(calls[2].options.params.path).toEqual({ workspace_id: "ws" });
});
