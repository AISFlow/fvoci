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
