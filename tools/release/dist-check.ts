// Release files and the user compose contract (docs/RELEASING.md).
//
//   bun tools/release/dist-check.ts render ROOT
//       Called by scripts/release-dist.sh with VERSION SHA REPOSITORY IMAGE
//       INDEX_DIGEST AMD64_DIGEST ARM64_DIGEST RUN_URL OUT COMPOSE_SOURCE
//       ENV_SOURCE GUIDE_SOURCE in the environment. Writes compose.yml,
//       env.example, INSTALL.md, release.json and RELEASE-NOTES.md into OUT;
//       the shell adds SHA256SUMS.
//   bun tools/release/dist-check.ts dockerfile DOCKERFILE
//       Exit 0 when the rust-build stage declares ARG FVOCI_BUILD_SHA, else 1
//       without output (the caller names the failure).
//   bun tools/release/dist-check.ts compose-config CONFIG_JSON IMAGE
//       Checks `docker compose config --format json` of the rendered compose
//       filled by scripts/release-preflight.sh.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import {
  ReleaseError,
  count,
  dumpJson,
  escapeRegExp,
  findAll,
  isRecord,
  readText,
  repr,
  sorted,
  truthy,
} from "./py.ts";

export type RenderArgs = {
  version: string;
  sha: string;
  repository: string;
  image: string;
  indexDigest: string;
  amd64Digest: string;
  arm64Digest: string;
  runUrl: string;
};

export type RenderSources = {
  composeSource: string;
  envSource: string;
  guideSource: string;
  compose: string;
  env: string;
  guide: string;
  notesTemplate: string;
};

// Files in the order they are written; error is set when rendering stopped
// after the files listed.
export type RenderResult = { files: Array<[string, string]>; error?: string };

const TRIAL_VERSION = /^0\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;
const FULL_SHA = /^[0-9a-f]{40}$/;
const DIGEST = /^sha256:[0-9a-f]{64}$/;

export function argumentErrors(args: RenderArgs): string[] {
  const errors: string[] = [];
  if (!TRIAL_VERSION.test(args.version)) errors.push(`version ${repr(args.version)} is not 0.y.z`);
  if (!FULL_SHA.test(args.sha)) errors.push("--sha must be a full commit SHA");
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(args.repository)) {
    errors.push("--repository must be owner/name");
  }
  if (!/^[a-z0-9.-]+(\/[a-z0-9._-]+)+$/.test(args.image)) {
    errors.push("--image must be a lowercase registry/repository without tag");
  }
  const digests: Array<[string, string]> = [
    ["index_digest", args.indexDigest],
    ["amd64_digest", args.amd64Digest],
    ["arm64_digest", args.arm64Digest],
  ];
  for (const [name, value] of digests) {
    if (!DIGEST.test(value)) errors.push(`${name} must be sha256:<64 hex>`);
  }
  if (!args.runUrl.startsWith("https://")) errors.push("--run-url must be an https URL");
  return errors;
}

// Line anchors: start of text or after LF, end of text or before LF.
const BOL = "(?<![^\\n])";
const EOL = "(?![^\\n])";

function minorOf(version: string): string {
  const dot = version.lastIndexOf(".");
  return dot < 0 ? version : version.slice(0, dot);
}

export function render(args: RenderArgs, sources: RenderSources): RenderResult {
  const { composeSource, envSource } = sources;
  const stop = (error: string, files: Array<[string, string]> = []) => ({ files, error });
  const imageRef = `${args.image}:${args.version}@${args.indexDigest}`;

  // The user compose names the product image once, as the YAML anchor every
  // service on the product image shares:
  //   x-fvoci-image: &fvoci-image ${FVOCI_IMAGE:-ghcr.io/aisflow/fvoci:<version>}
  const anchor = new RegExp(
    `${BOL}(x-fvoci-image:[ \\t]+&fvoci-image[ \\t]+)\\$\\{FVOCI_IMAGE:-${escapeRegExp(args.image)}:[^}\\s$]+\\}[ \\t]*${EOL}`,
    "g",
  );
  const anchors = findAll(anchor, sources.compose);
  if (anchors.length !== 1) {
    return stop(
      `${composeSource}: expected exactly one 'x-fvoci-image: &fvoci-image \${FVOCI_IMAGE:-${args.image}:...}' line, found ${String(anchors.length)}`,
    );
  }
  if (count(sources.compose, "FVOCI_IMAGE") !== 1) {
    return stop(`${composeSource}: FVOCI_IMAGE may appear only in the x-fvoci-image anchor`);
  }
  const rendered = sources.compose.replace(anchor, (_match, prefix: string) => prefix + imageRef);
  if (count(rendered, args.image) !== 1) {
    return stop(
      `${composeSource}: ${args.image} must be named only through the x-fvoci-image anchor`,
    );
  }
  if (!new RegExp(`${BOL}\\s+image:[ \\t]+\\*fvoci-image[ \\t]*${EOL}`).test(rendered)) {
    return stop(`${composeSource}: no service uses 'image: *fvoci-image'`);
  }
  // Everything left for Compose to interpolate comes from the user's .env:
  // each variable is required (${VAR:?message}, so an unfilled .env stops
  // Compose before any container starts) and env.example assigns exactly
  // these variables. No env_file: each service gets only the values it names.
  const assigned = findAll(new RegExp(`${BOL}([A-Z][A-Z0-9_]*)=`, "g"), sources.env);
  const assignedSet = new Set(assigned);
  if (assigned.length !== assignedSet.size) {
    return stop(`${envSource}: a variable is assigned twice`);
  }
  const used = new Set<string>();
  for (const ref of findAll(/(?<!\$)\$(?!\$)(\{[^}]*\}|[A-Za-z_][A-Za-z0-9_]*)/g, rendered)) {
    const match = /^\{([A-Z][A-Z0-9_]*):\?[^}]+\}$/.exec(ref);
    if (!match?.[1]) {
      return stop(`${composeSource}: every interpolation must be \${VAR:?message}; found $${ref}`);
    }
    used.add(match[1]);
  }
  const missing = sorted([...used].filter((name) => !assignedSet.has(name)));
  const unused = sorted(assigned.filter((name) => !used.has(name)));
  if (missing.length || unused.length) {
    return stop(
      `${envSource} must assign exactly the variables ${composeSource} reads: missing ${repr(missing)}, unused ${repr(unused)}`,
    );
  }
  if (new RegExp(`${BOL}\\s+env_file:`).test(rendered)) {
    return stop(`${composeSource}: the release compose must not use env_file`);
  }

  const files: Array<[string, string]> = [];
  const header =
    `# FVOCI ${args.version} (${args.sha}), rendered from ${composeSource}\n` +
    `# by the release workflow. The image is pinned by manifest digest.\n`;
  files.push(["compose.yml", header + rendered]);
  files.push(["env.example", sources.env]);
  files.push(["INSTALL.md", `<!-- FVOCI ${args.version} (${args.sha}) -->\n` + sources.guide]);

  const minor = minorOf(args.version);
  const record = {
    version: args.version,
    tag: `v${args.version}`,
    sourceSha: args.sha,
    image: imageRef,
    indexDigest: args.indexDigest,
    platforms: { "linux/amd64": args.amd64Digest, "linux/arm64": args.arm64Digest },
    composeSource,
    files: ["compose.yml", "env.example", "INSTALL.md"],
    // Publish order: the index is pushed by digest only; both smoke jobs pull
    // that digest anonymously; then the publish job applies the immutable
    // version tag and, when this is the newest 0.y release, moves the minor
    // tag; the GitHub release comes last.
    tags: { immutable: args.version, floating: minor },
    publishOrder: [
      "index-by-digest",
      "smoke-linux/amd64",
      "smoke-linux/arm64",
      `tag:${args.version}`,
      `tag:${minor} (if newest)`,
      "github-release",
    ],
  };
  files.push(["release.json", dumpJson(record, { indent: 2 }) + "\n"]);

  let notes = sources.notesTemplate;
  const placeholders: Array<[string, string]> = [
    ["@VERSION@", args.version],
    ["@SHA@", args.sha],
    ["@REPOSITORY@", args.repository],
    ["@IMAGE_REF@", imageRef],
    ["@AMD64_DIGEST@", args.amd64Digest],
    ["@ARM64_DIGEST@", args.arm64Digest],
    ["@RUN_URL@", args.runUrl],
  ];
  for (const [key, value] of placeholders) notes = notes.split(key).join(value);
  const left = sorted(new Set(findAll(/@[A-Z0-9_]+@/g, notes)));
  if (left.length) return stop(`release notes placeholders left: ${repr(left)}`, files);
  files.push(["RELEASE-NOTES.md", notes]);
  return { files };
}

// True when a stage named rust-build declares ARG FVOCI_BUILD_SHA.
export function declaresBuildSha(dockerfile: string): boolean {
  let stage: string | undefined;
  for (const line of dockerfile.split("\n")) {
    const start = /^\s*FROM\s+\S+(?:\s+AS\s+(\S+))?/i.exec(line);
    if (start) stage = (start[1] ?? "").toLowerCase();
    else if (stage === "rust-build" && /^\s*ARG\s+FVOCI_BUILD_SHA(=|\s|$)/.test(line)) return true;
  }
  return false;
}

export type ComposeConfigReport = { summary: string; problems: string[] };

// The filled .env gives each empty entry VAR the value preflight-VAR.
const GENERATED = /preflight-([A-Z][A-Z0-9_]*)/g;
const NEEDED_OUTSIDE_APP: Record<string, string[]> = {
  postgres: ["POSTGRES_PASSWORD"],
  meilisearch: ["MEILI_MASTER_KEY"],
};

function services(config: unknown): Record<string, Record<string, unknown>> {
  if (!isRecord(config) || !isRecord(config.services)) {
    throw new ReleaseError("compose config has no services mapping");
  }
  const out: Record<string, Record<string, unknown>> = {};
  for (const [name, spec] of Object.entries(config.services)) {
    if (!isRecord(spec)) throw new ReleaseError(`service ${name} is not a mapping`);
    out[name] = spec;
  }
  return out;
}

function list(value: unknown, what: string): unknown[] {
  if (value === undefined || value === null || value === false || value === "") return [];
  if (!Array.isArray(value)) throw new ReleaseError(`${what} is not a list`);
  return value;
}

function publishes8080(spec: Record<string, unknown>): boolean {
  return list(spec.ports, "ports").some((port) => isRecord(port) && port.target === 8080);
}

// Apps are the services publishing container port 8080.
export function appServices(config: unknown): string[] {
  const all = services(config);
  return sorted(Object.keys(all).filter((name) => publishes8080(all[name] ?? {})));
}

export function productServices(config: unknown, image: string): string[] {
  const all = services(config);
  return sorted(Object.keys(all).filter((name) => all[name]?.image === image));
}

export function checkComposeConfig(config: unknown, image: string): ComposeConfigReport {
  const all = services(config);
  const product = productServices(config, image);
  const apps = appServices(config);
  const problems: string[] = [];
  const [app] = apps;
  if (apps.length !== 1 || app === undefined) {
    problems.push(`expected one service publishing container port 8080, found ${repr(apps)}`);
  } else if (!product.includes(app)) {
    problems.push(`${app} (publishes 8080) does not use the product image`);
  }
  if (!Object.hasOwn(all, "postgres")) problems.push("no postgres service");
  for (const [name, spec] of Object.entries(all)) {
    if (truthy(spec.env_file)) problems.push(`${name} needs an env_file`);
  }
  // Outside the app, a service may receive only the value its own image
  // needs; nowhere may one appear outside `environment` (a command line is
  // readable by every host user).
  for (const [name, spec] of Object.entries(all)) {
    const environment = truthy(spec.environment) ? spec.environment : {};
    if (!isRecord(environment)) throw new ReleaseError(`${name} environment is not a mapping`);
    const received = sorted(
      new Set(
        Object.values(environment).flatMap((value) =>
          findAll(GENERATED, typeof value === "string" ? value : repr(value)),
        ),
      ),
    );
    const needed = NEEDED_OUTSIDE_APP[name] ?? [];
    const unneeded = apps.includes(name) ? [] : received.filter((v) => !needed.includes(v));
    if (unneeded.length) {
      problems.push(`${name} gets .env values it does not need: ${repr(unneeded)}`);
    }
    const rest = Object.fromEntries(Object.entries(spec).filter(([k]) => k !== "environment"));
    const elsewhere = sorted(new Set(findAll(GENERATED, JSON.stringify(rest))));
    if (elsewhere.length) {
      problems.push(`${name} has .env values outside its environment: ${repr(elsewhere)}`);
    }
  }
  return { summary: `product image services: ${repr(product)}; app: ${repr(apps)}`, problems };
}

function renderMain(root: string): number {
  const env = (name: string): string => {
    const value = process.env[name];
    if (value === undefined) throw new ReleaseError(`${name} is not set`);
    return value;
  };
  const args: RenderArgs = {
    version: env("VERSION"),
    sha: env("SHA"),
    repository: env("REPOSITORY"),
    image: env("IMAGE"),
    indexDigest: env("INDEX_DIGEST"),
    amd64Digest: env("AMD64_DIGEST"),
    arm64Digest: env("ARM64_DIGEST"),
    runUrl: env("RUN_URL"),
  };
  const errors = argumentErrors(args);
  if (errors.length) {
    process.stderr.write(errors.join("\n") + "\n");
    return 1;
  }
  const out = env("OUT");
  const composeSource = env("COMPOSE_SOURCE");
  const envSource = env("ENV_SOURCE");
  const guideSource = env("GUIDE_SOURCE");
  const result = render(args, {
    composeSource,
    envSource,
    guideSource,
    compose: readText(join(root, composeSource)),
    env: readText(join(root, envSource)),
    guide: readText(join(root, guideSource)),
    notesTemplate: readText(join(root, "scripts/release-notes-template.md")),
  });
  for (const [name, text] of result.files) writeFileSync(join(out, name), text);
  if (result.error !== undefined) {
    process.stderr.write(result.error + "\n");
    return 1;
  }
  return 0;
}

function composeConfigMain(configPath: string, image: string): number {
  const report = checkComposeConfig(JSON.parse(readFileSync(configPath, "utf8")), image);
  process.stdout.write(report.summary + "\n");
  if (report.problems.length) {
    process.stderr.write(report.problems.join("\n") + "\n");
    return 1;
  }
  return 0;
}

export function main(argv: string[]): number {
  const [command, ...rest] = argv;
  if (command === "render" && rest.length === 1 && rest[0]) return renderMain(rest[0]);
  if (command === "dockerfile" && rest.length === 1 && rest[0]) {
    return declaresBuildSha(readText(rest[0])) ? 0 : 1;
  }
  if (command === "compose-config" && rest.length === 2 && rest[0] && rest[1] !== undefined) {
    return composeConfigMain(rest[0], rest[1]);
  }
  process.stderr.write(
    "usage: dist-check.ts render ROOT | dockerfile DOCKERFILE | compose-config CONFIG_JSON IMAGE\n",
  );
  return 2;
}

if (import.meta.main) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`dist-check: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}
