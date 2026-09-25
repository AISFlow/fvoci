import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  SETTING_ENUM_OPTIONS,
  SETTINGS_CATALOG,
  SETTINGS_ENTRIES,
  optionKey,
  withoutAssets,
} from "./settings-catalog.ts";

const ko = JSON.parse(
  readFileSync(new URL("../../../../../packages/i18n/src/locales/ko.json", import.meta.url), "utf8"),
) as Record<string, string>;

test("every catalog label, help, group and enum option has a Korean string", () => {
  for (const [key, entry] of SETTINGS_ENTRIES) {
    for (const k of [entry.labelKey, entry.helpKey, entry.group]) {
      assert.ok(Object.hasOwn(ko, k), `missing ${k} for ${key}`);
    }
    for (const [leaf, widget] of Object.entries(entry.widgets)) {
      if (widget !== "enum") continue;
      const options = SETTING_ENUM_OPTIONS[`${key}.${leaf}`];
      assert.ok(options && options.length > 0, `no options for ${key}.${leaf}`);
      for (const value of options) {
        assert.ok(Object.hasOwn(ko, optionKey(key, leaf, value)), `missing option ${key}.${leaf}.${value}`);
      }
    }
  }
});

test("withoutAssets drops only the upload-route leaves", () => {
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

test("draft schemas mirror the server limits", () => {
  const share = SETTINGS_CATALOG.share.schema;
  assert.equal(share.safeParse({ enabled: true, defaultExpiresDays: 7, maxExpiresDays: 30 }).success, true);
  assert.equal(share.safeParse({ enabled: true, defaultExpiresDays: 40, maxExpiresDays: 30 }).success, false);
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
