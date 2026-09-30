import assert from "node:assert/strict";
import test from "node:test";
import { workspaceNameInput } from "@/lib/validators";
import { parseForm } from "./form.ts";

await test("parseForm: a blank workspace name is the shared too-small message", () => {
  const result = parseForm(workspaceNameInput, { name: "  " });
  assert.equal(result.ok, false);
  assert.equal(result.message.length > 0, true);
});

await test("parseForm: a name is accepted", () => {
  const result = parseForm(workspaceNameInput, { name: "Acme" });
  assert.deepEqual(result, { ok: true, data: { name: "Acme" } });
});
