import { type I18nKey, t } from "@fvoci/i18n";
import { reactive, ref } from "vue";

/** The part of a zod schema the form uses (lib/validators.ts). */
export interface FormSchema<O> {
  safeParse(
    data: unknown,
  ):
    | { success: true; data: O }
    | {
        success: false;
        error: { issues: readonly { path: readonly PropertyKey[]; message: string }[] };
      };
}

/** A validator message: `i18n:<key>` names a catalog message (lib/validators.ts). */
export function issueMessage(message: string): string {
  return message.startsWith("i18n:") ? t(message.slice(5) as I18nKey) : message;
}

/** Each field's first issue, as the React forms' zodResolver reports them. */
export function firstIssues<F extends string>(
  issues: readonly { path: readonly PropertyKey[]; message: string }[],
  fields: readonly F[],
): Partial<Record<F, string>> {
  const errors: Partial<Record<F, string>> = {};
  for (const issue of issues) {
    const field = String(issue.path[0] ?? "") as F;
    if (fields.includes(field) && !(field in errors)) errors[field] = issueMessage(issue.message);
  }
  return errors;
}

export interface AuthFormEnvironment {
  /** Moves focus to the input of `field` (a failed submit focuses the first invalid one). */
  focus(field: string): void;
}

const BROWSER: AuthFormEnvironment = {
  focus: (id) => document.getElementById(id)?.focus(),
};

function readFormFields<F extends string>(
  form: HTMLFormElement,
  fields: readonly F[],
): Record<F, string> {
  const data = new FormData(form);
  const values = {} as Record<F, string>;
  for (const field of fields) {
    const value = data.get(field);
    values[field] = typeof value === "string" ? value : "";
  }
  return values;
}

function isFormEventTarget(target: EventTarget | null | undefined): target is HTMLFormElement {
  return Boolean(target && (target as HTMLElement).tagName === "FORM");
}

/**
 * Form state for the sign-in pages, with the React forms' behaviour
 * (react-hook-form, mode onSubmit): nothing is validated before the first
 * submit; a submit validates every field with `schema`, shows each field's
 * first issue, focuses the first invalid field in `initial`'s order and runs
 * the handler only when all are valid, with the parsed (trimmed, normalized)
 * values; after a submit a changed field is validated again. Without a
 * schema the handler validates and sets errors itself, and a changed field
 * drops its error.
 *
 * Inputs stay uncontrolled (the DOM holds the value, like React
 * `defaultValues`). `values` is a copy used for validation; a submit reads
 * `FormData` when the listener receives a form event.
 *
 * `ids` maps a field to its input's element id, for the focus.
 */
export function useAuthForm<F extends string, O = Record<F, string>>(options: {
  initial: Record<F, string>;
  schema?: FormSchema<O>;
  ids?: Partial<Record<F, string>>;
  env?: AuthFormEnvironment;
}) {
  const env = options.env ?? BROWSER;
  const fields = Object.keys(options.initial) as F[];
  const values = reactive({ ...options.initial }) as Record<F, string>;
  const errors = reactive({}) as Partial<Record<F, string>>;
  const submitted = ref(false);
  const submitting = ref(false);

  function clearErrors(): void {
    for (const field of fields) delete errors[field];
  }

  function revalidate(field: F): void {
    if (!options.schema) {
      delete errors[field];
      return;
    }
    const result = options.schema.safeParse({ ...values });
    const message = result.success ? undefined : firstIssues(result.error.issues, [field])[field];
    if (message === undefined) delete errors[field];
    else errors[field] = message;
  }

  function onInput(field: F, value: string): void {
    values[field] = value;
    if (submitted.value) revalidate(field);
  }

  /** The submit listener: validates, then runs `onValid` with the parsed values. */
  function handleSubmit(
    onValid: (data: O) => Promise<void> | void,
  ): (event?: Event) => Promise<void> {
    return async (event) => {
      event?.preventDefault();
      if (isFormEventTarget(event?.target)) {
        Object.assign(values, readFormFields(event.target, fields));
      }
      submitting.value = true;
      try {
        clearErrors();
        let data: O;
        if (options.schema) {
          const result = options.schema.safeParse({ ...values });
          if (!result.success) {
            Object.assign(errors, firstIssues(result.error.issues, fields));
            const first = fields.find((field) => field in errors);
            const id = first === undefined ? undefined : options.ids?.[first];
            if (id) env.focus(id);
            return;
          }
          data = result.data;
        } else {
          data = { ...values } as O;
        }
        await onValid(data);
      } finally {
        submitted.value = true;
        submitting.value = false;
      }
    };
  }

  function setError(field: F, message: string): void {
    errors[field] = message;
  }

  return { values, errors, submitted, submitting, onInput, handleSubmit, setError };
}
