import { nextTick, reactive, ref, watch } from "vue";
import type { z } from "zod";
import { issueMessage } from "@/lib/issue-message";

/** String and checkbox fields; other widgets (file, number) stay outside this helper. */
type FieldValues = Record<string, string | boolean>;

/**
 * A small form over a zod schema, with the defaults the React forms get from
 * react-hook-form and zodResolver: nothing is checked until the first
 * submit; a submit that fails validation shows each field's first issue
 * (an `i18n:` message as its catalog string), focuses the first invalid
 * field, and from then on every change is checked again. A valid submit
 * hands the parsed values (trimmed, transformed) to `onValid`; `submitting`
 * holds while it runs. `reset` restores the defaults and the unchecked state.
 */
export function useZodForm<T extends FieldValues, O>(options: {
  /** A getter, so a schema that depends on props is read at submit time. */
  schema: () => z.ZodType<O, z.ZodTypeDef, T>;
  defaults: () => T;
  /** Element ids in the order the fields appear, for focusing the first invalid one. */
  fieldIds: Partial<Record<keyof T & string, string>>;
}) {
  const values = reactive({ ...options.defaults() }) as T;
  const errors = ref<Partial<Record<keyof T & string, string>>>({});
  const submitting = ref(false);
  let checked = false;

  function validate(): { ok: true; data: O } | { ok: false } {
    const parsed = options.schema().safeParse({ ...values });
    if (parsed.success) {
      errors.value = {};
      return { ok: true, data: parsed.data };
    }
    const next: Partial<Record<string, string>> = {};
    for (const issue of parsed.error.issues) {
      const field = issue.path[0];
      if (typeof field === "string" && next[field] === undefined) next[field] = issueMessage(issue.message);
    }
    errors.value = next as Partial<Record<keyof T & string, string>>;
    return { ok: false };
  }

  watch(
    () => ({ ...values }),
    () => {
      if (checked) validate();
    },
  );

  async function focusFirstError(): Promise<void> {
    await nextTick();
    const doc = globalThis.document;
    if (!doc) return;
    for (const [field, id] of Object.entries(options.fieldIds) as [string, string][]) {
      if (errors.value[field as keyof T & string] === undefined) continue;
      doc.getElementById(id)?.focus();
      return;
    }
  }

  async function submit(onValid: (data: O) => Promise<void> | void): Promise<void> {
    if (submitting.value) return;
    submitting.value = true;
    try {
      const result = validate();
      checked = true;
      if (!result.ok) {
        await focusFirstError();
        return;
      }
      await onValid(result.data);
    } finally {
      submitting.value = false;
    }
  }

  function reset(): void {
    Object.assign(values, options.defaults());
    errors.value = {};
    checked = false;
  }

  return { values, errors, submitting, submit, reset };
}

/** The text of an input or textarea event (for `:value` + `@input` bindings, which also update during IME composition). */
export function inputText(event: Event): string {
  return (event.target as HTMLInputElement | HTMLTextAreaElement).value;
}

/** The checked state of a checkbox event (for `:checked` + `@change`). */
export function inputChecked(event: Event): boolean {
  return (event.target as HTMLInputElement).checked;
}
