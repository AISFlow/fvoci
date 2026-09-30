import assert from "node:assert/strict";
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

test("hasLicenseText treats whitespace-only stubs as missing", () => {
  assert.equal(hasLicenseText("   "), false);
  assert.equal(hasLicenseText("upstream LICENSE body"), true);
});

test("applyBrowserLicenseSupplements fills only when upstream text is absent", () => {
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
  assert.doesNotThrow(() => assertBundledLicenseTexts(filled, supplements));
});

test("assertBundledLicenseTexts fails when a bundled id lacks text and supplement", () => {
  const supplements = loadBrowserLicenseManifest(manifestPath);
  const entries: LicenseEntry[] = [
    { name: "example-missing", version: "1.0.0", identifier: "MIT" },
  ];
  assert.throws(
    () => assertBundledLicenseTexts(entries, supplements),
    /no license text and no supplement/,
  );
});

test("finalizeBrowserOpenSourceNotice appends FVOCI LICENSE and editor font OFL files", () => {
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

test("finalizeBrowserOpenSourceNotice appends notices for copied runtime assets", () => {
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

test("published notice preserves web source provenance and full template permission text", () => {
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
