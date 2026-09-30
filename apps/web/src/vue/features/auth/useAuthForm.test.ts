import assert from "node:assert/strict";
import test from "node:test";
import { loginInput } from "@/lib/validators.ts";
import { firstIssues, issueMessage, useAuthForm } from "./useAuthForm.ts";

test("issueMessage looks up i18n: keys and leaves other text", () => {
  assert.equal(issueMessage("i18n:form.email"), "이메일 형식을 확인해 주세요.");
  assert.equal(issueMessage("plain"), "plain");
});

test("firstIssues keeps the first message per named field", () => {
  assert.deepEqual(
    firstIssues(
      [
        { path: ["email"], message: "i18n:form.email" },
        { path: ["email"], message: "second" },
        { path: ["password"], message: "i18n:form.too_small" },
        { path: ["other"], message: "skip" },
      ],
      ["email", "password"],
    ),
    {
      email: "이메일 형식을 확인해 주세요.",
      password: "값을 입력해 주세요.",
    },
  );
});

function form(env?: { focus(id: string): void }) {
  return useAuthForm({
    initial: { email: "", password: "" },
    schema: loginInput,
    ids: { email: "login-email", password: "login-password" },
    env: env ?? { focus: () => undefined },
  });
}

test("nothing is validated before the first submit", () => {
  const auth = form();
  auth.onInput("email", "not-an-email");
  assert.equal(auth.errors.email, undefined);
  assert.equal(auth.submitted.value, false);
});

test("a submit validates every field, focuses the first invalid, and skips the handler", async () => {
  const focused: string[] = [];
  const auth = form({ focus: (id) => focused.push(id) });
  let ran = false;
  await auth.handleSubmit(() => {
    ran = true;
  })();
  assert.equal(ran, false);
  assert.equal(auth.errors.email, "이메일 형식을 확인해 주세요.");
  assert.equal(auth.errors.password, "값을 입력해 주세요.");
  assert.deepEqual(focused, ["login-email"]);
  assert.equal(auth.submitted.value, true);
  assert.equal(auth.submitting.value, false);
});

test("a valid submit runs the handler with the parsed values", async () => {
  const auth = form();
  auth.onInput("email", "  user@example.com  ");
  auth.onInput("password", "secret");
  const seen: unknown[] = [];
  await auth.handleSubmit((data) => {
    seen.push(data);
  })();
  assert.deepEqual(seen, [{ email: "user@example.com", password: "secret" }]);
  assert.deepEqual({ ...auth.errors }, {});
});

test("after a submit a changed field is validated again", async () => {
  const auth = form();
  await auth.handleSubmit(() => undefined)();
  assert.equal(auth.errors.email, "이메일 형식을 확인해 주세요.");
  auth.onInput("email", "user@example.com");
  assert.equal(auth.errors.email, undefined);
  auth.onInput("email", "nope");
  assert.equal(auth.errors.email, "이메일 형식을 확인해 주세요.");
});

test("a form event calls preventDefault", async () => {
  const auth = form();
  const event = {
    preventDefault() {
      this.prevented = true;
    },
    prevented: false,
    target: null,
  };
  await auth.handleSubmit(() => undefined)(event as unknown as Event);
  assert.equal(event.prevented, true);
});

test("without a schema a changed field drops its error", async () => {
  const auth = useAuthForm({
    initial: { code: "" },
    ids: { code: "mfa-code" },
    env: { focus: () => undefined },
  });
  const onValid = auth.handleSubmit((data) => {
    if (data.code.trim() === "") auth.setError("code", "required");
  });
  await onValid();
  assert.equal(auth.errors.code, "required");
  auth.onInput("code", "123456");
  assert.equal(auth.errors.code, undefined);
});
