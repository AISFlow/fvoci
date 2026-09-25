// Web copy of the source settings catalog (packages/contracts/src/settings-catalog.ts)
// restricted to what the admin form draws: widgets per leaf, labels, safety and
// the draft schema that enables the save button. The Rust server
// (src/settings/catalog.rs) stays the authority; a rejected PATCH is a 400.
import { z } from "zod";

export type SettingsWidget =
  | "boolean"
  | "enum"
  | "number"
  | "text"
  | "list"
  | "duration"
  | "i18n_override"
  | "asset";

export type SettingsEntry = {
  readonly schema: z.ZodTypeAny;
  readonly safety: "live" | "restart_required";
  readonly widgets: Readonly<Record<string, SettingsWidget>>;
  readonly group: string;
  readonly labelKey: string;
  readonly helpKey: string;
  readonly confirmDestructive: boolean;
};

/** Server-sent messages an admin may override, with their allowed `{{vars}}`. */
export const OVERRIDABLE_MESSAGES: Readonly<Record<string, readonly string[]>> = {
  "seed.status.backlog": [],
  "seed.status.todo": [],
  "seed.status.in_progress": [],
  "seed.status.review": [],
  "seed.status.done": [],
  "seed.status.canceled": [],
  "mail.magic.login.subject": [],
  "mail.magic.reset.subject": [],
  "mail.magic.emailChange.subject": [],
  "mail.magic.emailChangeRequested.subject": [],
  "mail.magic.emailChangeCompleted.subject": [],
  "mail.magic.link.text": ["url", "minutes"],
  "mail.magic.emailChangeRequested.text": [],
  "mail.magic.emailChangeCompleted.text": [],
  "mail.invite.subject": [],
  "mail.identity.linked.subject": [],
  "mail.identity.linked.text": ["provider"],
  "mail.identity.unlinked.subject": [],
  "mail.identity.unlinked.text": ["provider"],
  "withdrawn.displayName": [],
  "task.duplicate.suffix": ["title"],
};

const MESSAGE_VAR = /\{\{(\w+)\}\}/g;
const NO_CONTROL = /^[^\p{Cc}]+$/u;

const messageOverrides = z
  .record(z.string(), z.string().trim().min(1).max(4_000))
  .superRefine((map, ctx) => {
    for (const [key, value] of Object.entries(map)) {
      const allowed = Object.hasOwn(OVERRIDABLE_MESSAGES, key)
        ? OVERRIDABLE_MESSAGES[key]
        : undefined;
      const vars = [...value.matchAll(MESSAGE_VAR)].map((m) => m[1] ?? "");
      const bad =
        allowed === undefined ||
        value.includes("<") ||
        vars.some((name) => !allowed.includes(name)) ||
        allowed.some((name) => !vars.includes(name));
      if (bad) ctx.addIssue({ code: "custom", message: "invalid input", path: [key] });
    }
  });

const brandText = z.string().trim().min(1).max(80).regex(NO_CONTROL, "invalid input");
const operatorText = z.string().trim().min(1).max(200).regex(NO_CONTROL, "invalid input");
const hostname = z
  .string()
  .trim()
  .min(1)
  .max(253)
  .regex(/^[a-z0-9.-]+$/, "invalid input");
const securityContact = z
  .string()
  .trim()
  .max(320)
  .regex(NO_CONTROL, "invalid input")
  .refine(
    (v) =>
      (v.startsWith("mailto:") && z.string().email().safeParse(v.slice(7)).success) ||
      (v.startsWith("https://") && v.length > "https://".length),
    { message: "invalid input" },
  );
const httpsUrl = z
  .string()
  .max(2048)
  .regex(NO_CONTROL, "invalid input")
  .url()
  .refine((v) => v.startsWith("https:"), { message: "invalid input" });

const brandingAsset = z
  .object({ key: z.string().uuid(), sha256: z.string(), mime: z.string() })
  .strict();

export const BRANDING_ASSET_MIME = ["image/png", "image/apng", "image/webp", "image/jpeg"] as const;
export const BRANDING_ASSET_KINDS = ["logo", "favicon"] as const;
export type BrandingAssetKind = (typeof BRANDING_ASSET_KINDS)[number];

export const SETTINGS_CATALOG = {
  branding: {
    schema: z
      .object({
        name: brandText,
        smtpFromDisplay: brandText.nullable(),
        logo: brandingAsset.nullable().optional(),
        favicon: brandingAsset.nullable().optional(),
        loginBrandText: brandText.nullable(),
      })
      .strict(),
    safety: "live",
    widgets: {
      name: "text",
      smtpFromDisplay: "text",
      logo: "asset",
      favicon: "asset",
      loginBrandText: "text",
    },
    group: "settings.group.branding",
    labelKey: "settings.branding.label",
    helpKey: "settings.branding.help",
    confirmDestructive: false,
  },
  "defaults.user": {
    schema: z
      .object({
        locale: z.enum(["ko"]),
        timezone: z.string().trim().min(1).max(64),
        weekStartsOn: z.union([z.literal(0), z.literal(1)]),
        textScale: z.union([z.literal(16), z.literal(18), z.literal(20)]),
      })
      .strict(),
    safety: "live",
    widgets: { locale: "enum", timezone: "text", weekStartsOn: "enum", textScale: "enum" },
    group: "settings.group.defaults",
    labelKey: "settings.defaults.user.label",
    helpKey: "settings.defaults.user.help",
    confirmDestructive: false,
  },
  auth: {
    schema: z.object({ passwordMinLength: z.number().int().min(10).max(128) }).strict(),
    safety: "live",
    widgets: { passwordMinLength: "number" },
    group: "settings.group.auth",
    labelKey: "settings.auth.label",
    helpKey: "settings.auth.help",
    confirmDestructive: false,
  },
  share: {
    schema: z
      .object({
        enabled: z.boolean(),
        defaultExpiresDays: z.number().int().min(1).max(365),
        maxExpiresDays: z.number().int().min(1).max(365),
      })
      .strict()
      .refine((v) => v.defaultExpiresDays <= v.maxExpiresDays, { message: "invalid input" }),
    safety: "live",
    widgets: { enabled: "boolean", defaultExpiresDays: "duration", maxExpiresDays: "duration" },
    group: "settings.group.share",
    labelKey: "settings.share.label",
    helpKey: "settings.share.help",
    confirmDestructive: false,
  },
  embed: {
    schema: z.object({ hosts: z.array(hostname).max(100) }).strict(),
    safety: "restart_required",
    widgets: { hosts: "list" },
    group: "settings.group.embed",
    labelKey: "settings.embed.label",
    helpKey: "settings.embed.help",
    confirmDestructive: false,
  },
  features: {
    schema: z.object({ ai: z.boolean() }).strict(),
    safety: "restart_required",
    widgets: { ai: "boolean" },
    group: "settings.group.features",
    labelKey: "settings.features.label",
    helpKey: "settings.features.help",
    confirmDestructive: false,
  },
  attachmentPreview: {
    schema: z.object({ mode: z.enum(["auto", "client", "server"]) }).strict(),
    safety: "live",
    widgets: { mode: "enum" },
    group: "settings.group.attachment",
    labelKey: "settings.attachmentPreview.label",
    helpKey: "settings.attachmentPreview.help",
    confirmDestructive: false,
  },
  i18n: {
    schema: z.object({ overrides: messageOverrides }).strict(),
    safety: "live",
    widgets: { overrides: "i18n_override" },
    group: "settings.group.i18n",
    labelKey: "settings.i18n.label",
    helpKey: "settings.i18n.help",
    confirmDestructive: false,
  },
  security: {
    schema: z.object({ contact: securityContact.nullable() }).strict(),
    safety: "live",
    widgets: { contact: "text" },
    group: "settings.group.security",
    labelKey: "settings.security.label",
    helpKey: "settings.security.help",
    confirmDestructive: false,
  },
  operator: {
    schema: z
      .object({
        businessName: operatorText.nullable(),
        representative: operatorText.nullable(),
        registrationNumber: operatorText.nullable(),
        mailOrderNumber: operatorText.nullable(),
        address: operatorText.nullable(),
        phone: operatorText.nullable(),
        supportEmail: z.string().email().max(320).nullable(),
        businessInfoUrl: httpsUrl.nullable(),
        hostingProvider: operatorText.nullable(),
      })
      .strict(),
    safety: "live",
    widgets: {
      businessName: "text",
      representative: "text",
      registrationNumber: "text",
      mailOrderNumber: "text",
      address: "text",
      phone: "text",
      supportEmail: "text",
      businessInfoUrl: "text",
      hostingProvider: "text",
    },
    group: "settings.group.operator",
    labelKey: "settings.operator.label",
    helpKey: "settings.operator.help",
    confirmDestructive: false,
  },
} as const satisfies Record<string, SettingsEntry>;

export type SettingsKey = keyof typeof SETTINGS_CATALOG;

export const SETTINGS_ENTRIES = Object.entries(SETTINGS_CATALOG) as [SettingsKey, SettingsEntry][];

/** Enum leaves and their values (the source derives these from the schemas). */
export const SETTING_ENUM_OPTIONS: Readonly<Record<string, readonly string[]>> = {
  "defaults.user.locale": ["ko"],
  "defaults.user.weekStartsOn": ["0", "1"],
  "defaults.user.textScale": ["16", "18", "20"],
  "attachmentPreview.mode": ["auto", "client", "server"],
};

/** Source `optionKey`: the i18n key of an enum option label. */
export function optionKey(key: string, leaf: string, value: string): string {
  return `settings.${key}.${leaf}.option.${value}`;
}

/** Source EE_GATED: setting key → enterprise feature that unlocks its card. */
export const EE_GATED: Partial<Record<SettingsKey, string>> = { branding: "branding" };

/** Asset leaves are written by the upload route only; the PATCH body omits them. */
export function withoutAssets(key: SettingsKey, doc: unknown): unknown {
  if (typeof doc !== "object" || doc === null || Array.isArray(doc)) return doc;
  const out: Record<string, unknown> = { ...(doc as Record<string, unknown>) };
  for (const [leaf, widget] of Object.entries(SETTINGS_CATALOG[key].widgets)) {
    if (widget === "asset") delete out[leaf];
  }
  return out;
}
