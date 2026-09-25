import { z } from "zod";

const slugPattern = /^[a-z0-9-]{2,32}$/;

const nfkcString = z.string().transform((value) => value.normalize("NFKC"));

const slugSchema = nfkcString.pipe(
  z.string().regex(slugPattern, "i18n:form.invalid"),
);

export const loginInput = z.object({
  email: z.string().trim().email("i18n:form.email"),
  password: z.string().min(1, "i18n:form.too_small"),
});

export const setupInput = z.object({
  email: z.string().trim().email("i18n:form.email"),
  password: z.string().min(10, "i18n:form.too_small"),
  givenName: z.string().trim().min(1, "i18n:form.too_small"),
  familyName: z.string().nullable().optional(),
  workspaceName: z.string().trim().min(1, "i18n:form.too_small"),
  workspaceSlug: slugSchema,
});

export const workspaceCreateInput = z.object({
  name: z.string().trim().min(1, "i18n:form.too_small"),
  slug: slugSchema,
});

export const workspaceNameInput = z.object({
  name: z.string().trim().min(1, "i18n:form.too_small"),
});

export const workspaceDeleteInput = z.object({
  confirmSlug: slugSchema,
});

export const invitationCreateInput = z.object({
  email: z.string().trim().email("i18n:form.email"),
  role: z.enum(["owner", "admin", "member", "guest"]),
});

const optionalEmail = z.preprocess(
  (value) => (typeof value === "string" && value.trim() === "" ? undefined : value),
  z.string().trim().email("i18n:form.email").optional(),
);

const optionalGivenName = z.preprocess(
  (value) => (typeof value === "string" && value.trim() === "" ? undefined : value),
  z.string().trim().min(1, "i18n:form.too_small").optional(),
);

const optionalPassword = z.preprocess(
  (value) => (typeof value === "string" && value === "" ? undefined : value),
  z.string().min(10, "i18n:form.too_small").optional(),
);

export const invitationAcceptInput = z.object({
  email: optionalEmail,
  givenName: optionalGivenName,
  familyName: z
    .string()
    .trim()
    .transform((value) => (value === "" ? undefined : value))
    .optional(),
  password: optionalPassword,
});

export const passwordResetInput = z.object({
  email: z.string().trim().email("i18n:form.email"),
});

export const passwordResetConfirmInput = z.object({
  token: z.string().min(1, "i18n:form.too_small"),
  newPassword: z.string().min(10, "i18n:form.too_small"),
});

export const apiTokenScope = z.enum([
  "documents.read",
  "documents.write",
  "tasks.read",
  "tasks.write",
  "projects.read",
  "projects.manage",
  "share.manage",
  "workspace.manage",
]);

export const apiTokenCreateInput = z.object({
  name: z.string().trim().min(1, "i18n:form.too_small").max(100, "i18n:form.too_big"),
  scopes: z.array(apiTokenScope).min(1, "i18n:token.scopes.required"),
  unlimited: z.boolean().optional(),
  service: z.boolean().optional(),
});

/** Source `webhookCreateInput`: an http(s) URL of at most 2048 chars and ≥1 event. */
export const webhookUrl = z
  .string()
  .trim()
  .min(1, "i18n:webhook.url.invalid")
  .max(2048, "i18n:webhook.url.invalid")
  .refine((value) => {
    try {
      const parsed = new URL(value);
      return parsed.protocol === "http:" || parsed.protocol === "https:";
    } catch {
      return false;
    }
  }, "i18n:webhook.url.invalid");

export const webhookCreateInput = z.object({
  url: webhookUrl,
  events: z.array(z.string().min(1)).min(1, "i18n:webhook.events.required").max(64),
});

/** GitHub issue link form: a task UUID, an `owner/name` repo and a positive issue number. */
export const githubIssueLinkForm = z.object({
  taskId: z.string().trim().uuid("i18n:github.issue.invalid"),
  repo: z
    .string()
    .trim()
    .regex(/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/, "i18n:github.issue.invalid"),
  issueNumber: z
    .string()
    .trim()
    .regex(/^[1-9][0-9]{0,9}$/, "i18n:github.issue.invalid")
    .refine((value) => Number(value) <= 2_147_483_647, "i18n:github.issue.invalid"),
});
