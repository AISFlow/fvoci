import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import {
  applyBrowserLicenseSupplements,
  assertBundledLicenseTexts,
  finalizeBrowserOpenSourceNotice,
  hasLicenseText,
  loadBrowserLicenseManifest,
  type LicenseEntry,
} from "./web-licenses.ts";

const repoRoot = path.resolve(import.meta.dirname, "../../../../");
const manifestPath = path.join(repoRoot, "third-party/browser-licenses/manifest.json");

/** Build regression guardrails only — not product license policy. */
const FORBIDDEN_NOTICE_PACKAGE_HEADINGS = ["@m2d/", "@playwright/", "vite - "] as const;

await test("hasLicenseText treats whitespace-only stubs as missing", () => {
  assert.equal(hasLicenseText("   "), false);
  assert.equal(hasLicenseText("upstream LICENSE body"), true);
});

await test("applyBrowserLicenseSupplements fills only when upstream text is absent", () => {
  const supplements = loadBrowserLicenseManifest(manifestPath);
  const entries: LicenseEntry[] = [
    {
      name: "qrcode-generator",
      version: "1.4.4",
      identifier: "MIT",
    },
    {
      name: "react",
      version: "19.3.0",
      identifier: "MIT",
      text: "preserve upstream bytes from Vite",
    },
  ];
  const filled = applyBrowserLicenseSupplements(entries, supplements, path.dirname(manifestPath));
  assert.equal(filled[1]?.text, "preserve upstream bytes from Vite");
  assert.match(filled[0]?.text ?? "", /Kazuhiko Arase/);
  assert.match(filled[0]?.supplementSource ?? "", /^https:\/\//);
  assert.doesNotThrow(() => {
    assertBundledLicenseTexts(filled, supplements);
  });
});

await test("assertBundledLicenseTexts fails when a bundled id lacks text and supplement", () => {
  const supplements = loadBrowserLicenseManifest(manifestPath);
  const entries: LicenseEntry[] = [
    { name: "example-missing", version: "1.0.0", identifier: "MIT" },
  ];
  assert.throws(() => {
    assertBundledLicenseTexts(entries, supplements);
  }, /no license text and no supplement/);
});

await test("locked Markdown browser dependencies retain full pinned upstream notices", () => {
  const supplements = loadBrowserLicenseManifest(manifestPath);
  const filled = applyBrowserLicenseSupplements(
    [
      { name: "launder", version: "1.7.1", identifier: "MIT" },
      { name: "remark-math", version: "6.0.0", identifier: "MIT" },
    ],
    supplements,
    path.dirname(manifestPath),
  );
  assertBundledLicenseTexts(filled, supplements);
  // Exact relocated upstream PROJECT notice, including its conditional section.
  // This is not a package-specific notice or a claim that vue-color is bundled.
  assert.equal(
    createHash("sha256")
      .update(filled[0]?.text ?? "")
      .digest("hex"),
    "fd8fe2fc75626c3b35be2b78129503cf09df127449b07c5fbf4c1713e3418390",
  );
  assert.equal(
    filled[0]?.supplementSource,
    "https://raw.githubusercontent.com/apostrophecms/apostrophe/e9b0ab0849a5dfea0f75335fbdf99b5c6bf9e4b3/packages/apostrophe/LICENSE.md",
  );
  assert.equal(
    filled[1]?.supplementSource,
    "https://raw.githubusercontent.com/remarkjs/remark-math/d5d0660b150810a535bbb07eac6cc96a4510aa24/license",
  );
  assert.match(filled[1].text ?? "", /Copyright/);
  assert.match(filled[1].text ?? "", /Permission is hereby granted/);
  const notice = finalizeBrowserOpenSourceNotice(JSON.stringify(filled), repoRoot, manifestPath);
  for (const entry of filled) assert.ok(notice.includes(entry.text ?? "missing"));
});

await test("finalizeBrowserOpenSourceNotice appends FVOCI LICENSE and editor font OFL files", () => {
  const sampleJson = JSON.stringify([
    {
      name: "react",
      version: "19.3.0",
      identifier: "MIT",
      text: "Permission is hereby granted, free of charge, to any person obtaining a copy\nTHE SOFTWARE IS PROVIDED",
    },
    {
      name: "qrcode-generator",
      version: "1.4.4",
      identifier: "MIT",
    },
  ]);
  const notice = finalizeBrowserOpenSourceNotice(sampleJson, repoRoot, manifestPath);
  assert.match(notice, /## react - 19\.3\.0 \(MIT\)/);
  assert.match(notice, /Kazuhiko Arase/);
  assert.match(
    notice,
    /Supplement source: https:\/\/raw\.githubusercontent\.com\/kazuhikoarase\/qrcode-generator/,
  );
  assert.match(notice, /## FVOCI source: LICENSE/);
  assert.match(notice, /## FVOCI source: packages\/editor\/src\/fonts\/NotoSansKR-OFL\.txt/);
  assert.match(notice, /SIL OPEN FONT LICENSE/);
  for (const forbidden of FORBIDDEN_NOTICE_PACKAGE_HEADINGS) {
    assert.doesNotMatch(notice, new RegExp(forbidden.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  }
});

await test("finalizeBrowserOpenSourceNotice appends notices for copied runtime assets", () => {
  const sampleJson = JSON.stringify([
    { name: "react", version: "19.3.0", identifier: "MIT", text: "Permission is hereby granted" },
  ]);
  const notice = finalizeBrowserOpenSourceNotice(sampleJson, repoRoot, manifestPath, [
    {
      title: "pdfjs-dist 6.3.289 runtime data: standard_fonts/LICENSE_FOXIT",
      text: "Foxit notice text",
    },
  ]);
  assert.match(notice, /## FVOCI source: LICENSE/);
  assert.match(
    notice,
    /## pdfjs-dist 6\.3\.289 runtime data: standard_fonts\/LICENSE_FOXIT\n\nFoxit notice text\n/,
  );
});

await test("published notice preserves web source provenance and full template permission text", () => {
  const sourceNotice = fs.readFileSync(path.join(repoRoot, "apps/web/NOTICE.md"), "utf8").trim();
  const notice = finalizeBrowserOpenSourceNotice(
    JSON.stringify([
      { name: "vue", version: "3.5.43", identifier: "MIT", text: "Vue dependency license" },
    ]),
    repoRoot,
    manifestPath,
  );
  assert.ok(notice.includes(`## FVOCI source: apps/web/NOTICE.md\n\n${sourceNotice}\n`));
  assert.match(notice, /393795261322b916e588043cf94feca999175843/);
  assert.match(notice, /57e8a76e85ac382f2dd75946aa450afb1b3e4b0d/);
  assert.match(notice, /Copyright \(c\) 2025 Nuxt UI Templates/);
  assert.match(
    notice,
    /OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE\nSOFTWARE\./,
  );
});
