import assert from "node:assert/strict";
import test from "node:test";
import { githubIssueLinkForm, webhookCreateInput } from "@/lib/validators";
import { WEBHOOK_EVENTS, webhookCreateProblemKey, webhookEventLabel } from "./webhook-events.ts";

test("every offered webhook event has a Korean label; unknown verbs pass through", () => {
  for (const verb of WEBHOOK_EVENTS) {
    assert.notEqual(webhookEventLabel(verb), verb, verb);
  }
  assert.equal(webhookEventLabel("project.created"), "프로젝트 생성");
  assert.equal(webhookEventLabel("custom.verb"), "custom.verb");
});

test("webhook form accepts http(s) URLs up to 2048 chars with at least one event", () => {
  assert.equal(
    webhookCreateInput.safeParse({ url: "https://example.com/hook", events: ["task.created"] }).success,
    true,
  );
  assert.equal(
    webhookCreateInput.safeParse({ url: "http://127.0.0.1:8080/hook", events: ["task.created"] }).success,
    true,
  );
  for (const url of ["", "ftp://example.com/x", "not a url", `https://e.com/${"a".repeat(2048)}`]) {
    assert.equal(webhookCreateInput.safeParse({ url, events: ["task.created"] }).success, false, url);
  }
  assert.equal(webhookCreateInput.safeParse({ url: "https://example.com", events: [] }).success, false);
});

test("webhook create problems map to specific messages", () => {
  assert.equal(webhookCreateProblemKey("integration_unavailable", null), "webhook.unavailable");
  assert.equal(webhookCreateProblemKey("invalid_input", "/url"), "webhook.url.refused");
  assert.equal(webhookCreateProblemKey("invalid_input", "/events"), "webhook.events.required");
  assert.equal(webhookCreateProblemKey("invalid_input", null), null);
  assert.equal(webhookCreateProblemKey("not_found", "/url"), null);
});

test("github issue link form requires a task UUID, owner/name repo and positive number", () => {
  const ok = {
    taskId: "0190a3b2-1c2d-7e3f-8a4b-5c6d7e8f9a0b",
    repo: "octo-org/hello.world",
    issueNumber: "42",
  };
  assert.equal(githubIssueLinkForm.safeParse(ok).success, true);
  assert.equal(githubIssueLinkForm.safeParse({ ...ok, taskId: "x" }).success, false);
  assert.equal(githubIssueLinkForm.safeParse({ ...ok, repo: "noslash" }).success, false);
  assert.equal(githubIssueLinkForm.safeParse({ ...ok, issueNumber: "0" }).success, false);
  assert.equal(githubIssueLinkForm.safeParse({ ...ok, issueNumber: "2147483648" }).success, false);
});
