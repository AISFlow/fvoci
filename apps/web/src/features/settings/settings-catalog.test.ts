import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  SETTING_ENUM_OPTIONS,
  SETTINGS_CATALOG,
  SETTINGS_ENTRIES,
  attachmentTransferView,
  optionKey,
  withoutAssets,
} from "./settings-catalog.ts";

const ko = JSON.parse(
  readFileSync(
    new URL("../../../../../packages/i18n/src/locales/ko.json", import.meta.url),
    "utf8",
  ),
) as Record<string, string>;

await test("every catalog label, help, group and enum option has a Korean string", () => {
  for (const [key, entry] of SETTINGS_ENTRIES) {
    for (const k of [entry.labelKey, entry.helpKey, entry.group]) {
      assert.ok(Object.hasOwn(ko, k), `missing ${k} for ${key}`);
    }
    for (const [leaf, widget] of Object.entries(entry.widgets)) {
      if (widget !== "enum") continue;
      const options = SETTING_ENUM_OPTIONS[`${key}.${leaf}`];
      assert.ok(options && options.length > 0, `no options for ${key}.${leaf}`);
      for (const value of options) {
        assert.ok(
          Object.hasOwn(ko, optionKey(key, leaf, value)),
          `missing option ${key}.${leaf}.${value}`,
        );
      }
    }
  }
});

await test("withoutAssets drops only the upload-route leaves", () => {
  assert.deepEqual(
    withoutAssets("branding", {
      name: "N",
      smtpFromDisplay: null,
      logo: null,
      favicon: null,
      loginBrandText: null,
    }),
    { name: "N", smtpFromDisplay: null, loginBrandText: null },
  );
  assert.deepEqual(withoutAssets("share", { enabled: true }), { enabled: true });
});

await test("draft schemas mirror the server limits", () => {
  const share = SETTINGS_CATALOG.share.schema;
  assert.equal(
    share.safeParse({ enabled: true, defaultExpiresDays: 7, maxExpiresDays: 30 }).success,
    true,
  );
  assert.equal(
    share.safeParse({ enabled: true, defaultExpiresDays: 40, maxExpiresDays: 30 }).success,
    false,
  );
  const i18n = SETTINGS_CATALOG.i18n.schema;
  assert.equal(i18n.safeParse({ overrides: { "mail.invite.subject": "초대" } }).success, true);
  assert.equal(i18n.safeParse({ overrides: { "mail.invite.subject": "<b>" } }).success, false);
  assert.equal(i18n.safeParse({ overrides: { "mail.magic.link.text": "{{url}}" } }).success, false);
  assert.equal(i18n.safeParse({ overrides: { toString: "x" } }).success, false);
  const op = SETTINGS_CATALOG.operator.schema;
  const empty = {
    businessName: null,
    representative: null,
    registrationNumber: null,
    mailOrderNumber: null,
    address: null,
    phone: null,
    supportEmail: null,
    businessInfoUrl: null,
    hostingProvider: null,
  };
  assert.equal(op.safeParse({ ...empty, businessInfoUrl: "https://example.com" }).success, true);
  assert.equal(op.safeParse({ ...empty, businessInfoUrl: "javascript:alert(1)" }).success, false);
});

await test("attachmentTransfer accepts only the two modes the server knows", () => {
  const schema = SETTINGS_CATALOG.attachmentTransfer.schema;
  assert.equal(schema.safeParse({ mode: "proxy" }).success, true);
  assert.equal(schema.safeParse({ mode: "presigned" }).success, true);
  assert.equal(schema.safeParse({ mode: "direct" }).success, false);
  assert.equal(schema.safeParse({ mode: "proxy", ttl: 5 }).success, false);
  assert.deepEqual(SETTING_ENUM_OPTIONS["attachmentTransfer.mode"], ["proxy", "presigned"]);
});

await test("attachmentTransfer card: effective mode, unavailable reason and blocked value", () => {
  const status = (over: Record<string, unknown>) => ({
    effective: "proxy" as const,
    source: "default" as const,
    presignedAvailable: true,
    unavailableReason: null,
    blocked: false,
    ...over,
  });
  const capable = attachmentTransferView(status({ effective: "presigned", source: "stored" }));
  assert.equal(capable.effectiveOptionKey, "settings.attachmentTransfer.mode.option.presigned");
  assert.equal(capable.disabledOptions.size, 0);
  assert.equal(capable.unavailableKey, null);
  assert.equal(capable.blocked, false);

  for (const reason of ["storage_local", "public_endpoint_missing"] as const) {
    const view = attachmentTransferView(
      status({ presignedAvailable: false, unavailableReason: reason }),
    );
    assert.deepEqual([...view.disabledOptions], ["presigned"]);
    assert.equal(view.unavailableKey, `settings.attachmentTransfer.unavailable.${reason}`);
    assert.ok(Object.hasOwn(ko, view.unavailableKey), reason);
  }

  const blocked = attachmentTransferView(
    status({
      source: "stored",
      presignedAvailable: false,
      unavailableReason: "public_endpoint_missing",
      blocked: true,
    }),
  );
  assert.equal(blocked.blocked, true);
  assert.equal(blocked.effectiveOptionKey, "settings.attachmentTransfer.mode.option.proxy");
  for (const key of [
    "settings.attachmentTransfer.blocked",
    "settings.attachmentTransfer.effective",
  ]) {
    assert.ok(Object.hasOwn(ko, key), key);
  }
  assert.deepEqual(attachmentTransferView(undefined), {
    effectiveOptionKey: null,
    disabledOptions: new Set(),
    unavailableKey: null,
    blocked: false,
  });
});
