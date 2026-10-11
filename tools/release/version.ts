// Release version policy (scripts/release-check-version.sh): the root
// Cargo.toml [package] version is the single source; every other place that
// carries the product version must match it.
//
//   bun tools/release/version.ts ROOT     (TAG and IMAGE_TAG in the environment)
// Prints the version on success; otherwise one "release-check-version: ..."
// line per problem on stderr and exit 1.
import { join } from "node:path";
import { ReleaseError, isRecord, readText, repr } from "./py.ts";

export type VersionSources = {
  cargoToml: string;
  cargoLock: string;
  openapiJson: string;
  openapiRs: string;
  webPackageJson: string;
};

function table(value: unknown, what: string): Record<string, unknown> {
  if (!isRecord(value)) throw new ReleaseError(`${what} is not a table`);
  return value;
}

export function versionErrors(
  sources: VersionSources,
  tag: string,
  imageTag: string,
): { version: unknown; errors: string[] } {
  const errors: string[] = [];
  const version = table(
    table(Bun.TOML.parse(sources.cargoToml), "Cargo.toml").package,
    "[package]",
  ).version;
  if (typeof version !== "string" || !/^0\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(version)) {
    errors.push(`Cargo.toml version ${repr(version)} is not a 0.y.z trial version`);
  }

  const lock = table(Bun.TOML.parse(sources.cargoLock), "Cargo.lock").package;
  if (!Array.isArray(lock)) throw new ReleaseError("Cargo.lock has no [[package]] entries");
  const locked = lock
    .map((entry) => table(entry, "Cargo.lock [[package]]"))
    .filter((entry) => entry.name === "fvoci-server")
    .map((entry) => entry.version);
  if (locked.length !== 1 || locked[0] !== version) {
    errors.push(`Cargo.lock fvoci-server ${repr(locked)} != Cargo.toml ${String(version)}`);
  }

  // The committed OpenAPI document is generated from the Rust DTOs
  // (scripts/generate-api.sh); its info.version comes from src/api/openapi.rs.
  const openapi = table(JSON.parse(sources.openapiJson), "apps/web/openapi.json");
  const infoVersion = table(openapi.info, "openapi info").version;
  if (infoVersion !== version) {
    errors.push(`apps/web/openapi.json info.version ${repr(infoVersion)} != ${String(version)}`);
  }
  const literal = /info\([^)]*?\bversion\s*=\s*"([^"]*)"/s.exec(sources.openapiRs);
  if (literal?.[1] !== undefined && literal[1] !== version) {
    errors.push(`src/api/openapi.rs info version ${repr(literal[1])} != ${String(version)}`);
  }

  // apps/web is private and carries no version today; if one appears it must match.
  const pkg = table(JSON.parse(sources.webPackageJson), "apps/web/package.json");
  if (Object.hasOwn(pkg, "version") && pkg.version !== version) {
    errors.push(`apps/web/package.json version ${repr(pkg.version)} != ${String(version)}`);
  }

  if (tag && tag !== `v${String(version)}`) errors.push(`tag ${repr(tag)} != v${String(version)}`);
  if (imageTag && imageTag !== version) {
    errors.push(`image tag ${repr(imageTag)} != ${String(version)}`);
  }
  return { version, errors };
}

export function readSources(root: string): VersionSources {
  return {
    cargoToml: readText(join(root, "Cargo.toml")),
    cargoLock: readText(join(root, "Cargo.lock")),
    openapiJson: readText(join(root, "apps/web/openapi.json")),
    openapiRs: readText(join(root, "src/api/openapi.rs")),
    webPackageJson: readText(join(root, "apps/web/package.json")),
  };
}

if (import.meta.main) {
  try {
    const [root] = process.argv.slice(2);
    if (!root) throw new ReleaseError("usage: version.ts ROOT");
    const { version, errors } = versionErrors(
      readSources(root),
      process.env.TAG ?? "",
      process.env.IMAGE_TAG ?? "",
    );
    if (errors.length) {
      process.stderr.write(errors.map((err) => `release-check-version: ${err}\n`).join(""));
      process.exitCode = 1;
    } else {
      process.stdout.write(`${String(version)}\n`);
    }
  } catch (error) {
    process.stderr.write(
      `release-check-version: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    process.exitCode = 1;
  }
}
