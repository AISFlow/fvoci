import assert from "node:assert/strict";
import test from "node:test";
import { effectScope } from "vue";
import { z } from "zod";
import { inputChecked, inputText, useZodForm } from "./useZodForm.ts";

function mount<T>(use: () => T): { result: T; stop: () => void } {
  const scope = effectScope();
  const result = scope.run(use) as T;
  return { result, stop: () => scope.stop() };
}

const emailSchema = z.object({
  newEmail: z.string().trim().email("i18n:form.email"),
});

test("nothing is checked until the first submit", async () => {
  const { result, stop } = mount(() =>
    useZodForm({
      schema: () => emailSchema,
      defaults: () => ({ newEmail: "" }),
      fieldIds: { newEmail: "settings-new-email" },
    }),
  );
  try {
    result.values.newEmail = "not-an-email";
    await Promise.resolve();
    assert.deepEqual(result.errors.value, {});
    await result.submit(async () => {
      assert.fail("invalid values must not be submitted");
    });
    assert.equal(result.errors.value.newEmail, "이메일 형식을 확인해 주세요.");
  } finally {
    stop();
  }
});

test("a valid submit hands the parsed values and reset restores the unchecked state", async () => {
  const { result, stop } = mount(() =>
    useZodForm({
      schema: () => emailSchema,
      defaults: () => ({ newEmail: "" }),
      fieldIds: { newEmail: "settings-new-email" },
    }),
  );
  try {
    const received: string[] = [];
    result.values.newEmail = "  user@example.com ";
    await result.submit(async (data) => {
      received.push(data.newEmail);
    });
    assert.deepEqual(received, ["user@example.com"]);
    assert.deepEqual(result.errors.value, {});
    result.reset();
    assert.equal(result.values.newEmail, "");
    result.values.newEmail = "still-bad";
    await Promise.resolve();
    assert.deepEqual(result.errors.value, {}, "reset returns to the unchecked state");
  } finally {
    stop();
  }
});

test("a second submit is ignored while the first is still running", async () => {
  const { result, stop } = mount(() =>
    useZodForm({
      schema: () => emailSchema,
      defaults: () => ({ newEmail: "a@b.co" }),
      fieldIds: { newEmail: "settings-new-email" },
    }),
  );
  try {
    let release: () => void = () => undefined;
    const first = result.submit(
      () =>
        new Promise<void>((resolve) => {
          release = resolve;
        }),
    );
    const second = result.submit(async () => {
      assert.fail("the in-flight submit must not be joined");
    });
    assert.equal(result.submitting.value, true);
    release();
    await first;
    await second;
    assert.equal(result.submitting.value, false);
  } finally {
    stop();
  }
});

test("input helpers read text and checkbox events", () => {
  const text = { target: { value: "hello" } } as unknown as Event;
  const box = { target: { checked: true } } as unknown as Event;
  assert.equal(inputText(text), "hello");
  assert.equal(inputChecked(box), true);
});
