import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

const dir = import.meta.dirname;

function source(file: string): string {
  return readFileSync(path.join(dir, file), "utf8");
}

await test("the Vue trash page restores wiki vs project documents on the same POSTs as React", () => {
  const page = source("../../pages/TrashPage.vue");
  assert.match(page, /trashQuery\(workspaceId\.value\)/);
  assert.match(page, /enabled: Boolean\(workspaceId\.value\)/);
  assert.match(
    page,
    /api\.POST\(\s*"\/api\/v1\/workspaces\/\{workspace_id\}\/projects\/\{project_id\}\/documents\/\{document_id\}\/restore"/,
  );
  assert.match(
    page,
    /api\.POST\("\/api\/v1\/workspaces\/\{workspace_id\}\/documents\/\{document_id\}\/restore"/,
  );
  assert.match(page, /item\.projectId/);
  assert.match(page, /wikiPath\(slug\)/);
  assert.doesNotMatch(page, /RouterLink/);
  assert.doesNotMatch(page, /from ["']react["']/);
  assert.doesNotMatch(page, /from ["']@tanstack\/react-query["']/);
});
