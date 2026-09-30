import fs from "node:fs";
import path from "node:path";

export type LicenseEntry = {
  name: string;
  version: string;
  identifier?: string;
  text?: string;
  /** Set when a target-owned supplement replaces missing upstream file text. */
  supplementSource?: string;
};

export type BrowserLicenseSupplement = {
  file: string;
  source: string;
};

export type BrowserLicenseManifest = Readonly<Partial<Record<string, BrowserLicenseSupplement>>>;

export function hasLicenseText(text: string | undefined): boolean {
  return (text?.trim() ?? "").length > 0;
}

export function parseLicenseJson(raw: string): LicenseEntry[] {
  const parsed: unknown = JSON.parse(raw);
  if (!Array.isArray(parsed)) {
    throw new Error("Vite license JSON must be an array");
  }
  return parsed.map((entry) => {
    if (typeof entry !== "object" || entry === null) {
      throw new Error("Vite license entry must be an object");
    }
    const { name, version, identifier, text } = entry as LicenseEntry;
    if (typeof name !== "string" || typeof version !== "string") {
      throw new Error("Vite license entry requires name and version");
    }
    return {
      name,
      version,
      ...(typeof identifier === "string" ? { identifier } : {}),
      ...(typeof text === "string" ? { text: text.trim() } : {}),
    };
  });
}

export function loadBrowserLicenseManifest(manifestPath: string): BrowserLicenseManifest {
  const raw = fs.readFileSync(manifestPath, "utf8");
  const parsed: unknown = JSON.parse(raw);
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new Error(`${manifestPath} must be a JSON object`);
  }
  return parsed as BrowserLicenseManifest;
}

export function licenseEntryId(entry: LicenseEntry): string {
  return `${entry.name}@${entry.version}`;
}

export function applyBrowserLicenseSupplements(
  entries: LicenseEntry[],
  supplements: BrowserLicenseManifest,
  supplementsRoot: string,
): LicenseEntry[] {
  return entries.map((entry) => {
    if (hasLicenseText(entry.text)) {
      return entry;
    }
    const id = licenseEntryId(entry);
    const supplement = supplements[id];
    if (!supplement) {
      return entry;
    }
    const supplementPath = path.join(supplementsRoot, supplement.file);
    const text = fs.readFileSync(supplementPath, "utf8").trim();
    if (!hasLicenseText(text)) {
      throw new Error(`${id}: supplement at ${supplementPath} is empty`);
    }
    return { ...entry, text, supplementSource: supplement.source };
  });
}

export function isPrivateWorkspacePackage(entry: LicenseEntry): boolean {
  return entry.name.startsWith("@fvoci/");
}

export function assertBundledLicenseTexts(
  entries: LicenseEntry[],
  supplements: BrowserLicenseManifest,
): void {
  const missing: string[] = [];
  for (const entry of entries) {
    if (isPrivateWorkspacePackage(entry)) {
      continue;
    }
    const id = licenseEntryId(entry);
    if (hasLicenseText(entry.text)) {
      continue;
    }
    if (supplements[id]) {
      missing.push(`${id}: supplement present but text still missing after apply`);
      continue;
    }
    missing.push(`${id}: bundled dependency has no license text and no supplement`);
  }
  if (missing.length > 0) {
    throw new Error(`Browser open-source notice is incomplete:\n${missing.join("\n")}`);
  }
}

export type BundledSourceNotice = {
  title: string;
  text: string;
};

export function readBundledSourceNotices(repoRoot: string): BundledSourceNotice[] {
  const files = [
    "LICENSE",
    "apps/web/NOTICE.md",
    "packages/editor/src/fonts/NotoSansKR-OFL.txt",
    "packages/editor/src/fonts/NotoSansCJK-OFL.txt",
    "packages/editor/src/fonts/NotoEmoji-OFL.txt",
    "packages/editor/src/fonts/README.md",
  ] as const;
  return files.map((relative) => {
    const absolute = path.join(repoRoot, relative);
    return {
      title: `FVOCI source: ${relative}`,
      text: fs.readFileSync(absolute, "utf8").trim(),
    };
  });
}

export function licenseEntriesToPublicMarkdown(
  entries: LicenseEntry[],
  bundled: BundledSourceNotice[],
): string {
  const published = entries.filter((entry) => !isPrivateWorkspacePackage(entry));
  if (published.length === 0) {
    throw new Error("Browser bundle produced no dependency license entries");
  }
  let text = `# Licenses

The app bundles dependencies which contain the following licenses:
`;
  for (const license of published) {
    const nameAndVersionText = `${license.name} - ${license.version}`;
    const identifierText = license.identifier ? ` (${license.identifier})` : "";
    text += `\n## ${nameAndVersionText}${identifierText}\n`;
    if (license.supplementSource) {
      text += `\nSupplement source: ${license.supplementSource}\n`;
    }
    if (license.text) {
      text += `\n${license.text.trim()}\n`;
    }
  }
  for (const extra of bundled) {
    text += `\n## ${extra.title}\n\n${extra.text}\n`;
  }
  return text;
}

export function finalizeBrowserOpenSourceNotice(
  licenseJson: string,
  repoRoot: string,
  manifestPath: string,
  assetNotices: BundledSourceNotice[] = [],
): string {
  const supplementsRoot = path.dirname(manifestPath);
  const supplements = loadBrowserLicenseManifest(manifestPath);
  let entries = parseLicenseJson(licenseJson);
  entries = applyBrowserLicenseSupplements(entries, supplements, supplementsRoot);
  assertBundledLicenseTexts(entries, supplements);
  const bundled = readBundledSourceNotices(repoRoot);
  return licenseEntriesToPublicMarkdown(entries, [...bundled, ...assetNotices]);
}
