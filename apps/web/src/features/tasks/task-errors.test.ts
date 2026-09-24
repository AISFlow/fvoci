import assert from "node:assert/strict";
import test from "node:test";
import { ProblemError } from "../../lib/api.ts";
import { taskFieldValidationMessage, taskMutationErrorMessage } from "./task-errors.ts";

test("taskMutationErrorMessage maps version and WIP conflicts in Korean", () => {
  assert.match(
    taskMutationErrorMessage(new ProblemError(409, "document_version_mismatch")),
    /다른 곳에서 먼저 수정되었습니다/,
  );
  assert.match(
    taskMutationErrorMessage(new ProblemError(409, "wip_limit_exceeded")),
    /진행 중 제한/,
  );
  assert.match(
    taskMutationErrorMessage(new ProblemError(409, "task_hierarchy_violation")),
    /상하위 관계/,
  );
});

test("taskFieldValidationMessage returns field-specific Korean copy", () => {
  assert.match(taskFieldValidationMessage("dueDate"), /YYYY-MM-DD/);
  assert.match(taskFieldValidationMessage("estimate"), /추정치/);
  assert.match(taskFieldValidationMessage("parent"), /상위 태스크/);
});
