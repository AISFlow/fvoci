// CI-only policy around Bun's YAML/TOML/tar implementations and GNU tar.
// No extraction to disk and no matched credential values are printed.
import { basename, join } from "node:path";
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { gunzipSync } from "node:zlib";

class PolicyError extends Error {}
function imageInterrupts() {
  const controller = new AbortController();
  const interrupt = (code: number) => {
    process.exitCode = code;
    controller.abort();
  };
  const onInt = () => interrupt(130),
    onTerm = () => interrupt(143);
  process.on("SIGINT", onInt);
  process.on("SIGTERM", onTerm);
  return {
    signal: controller.signal,
    check: () => check(!controller.signal.aborted, "image operation interrupted"),
    close: () => {
      process.off("SIGINT", onInt);
      process.off("SIGTERM", onTerm);
    },
  };
}
function check(condition: unknown, message: string): asserts condition {
  if (!condition) throw new PolicyError(message);
}
// Parsed JSON/YAML/TOML is untrusted: read it only through these narrowing helpers.
function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function at(value: unknown, ...path: string[]): unknown {
  let current = value;
  for (const key of path) {
    if (!isObject(current) || !Object.hasOwn(current, key)) return undefined;
    current = current[key];
  }
  return current;
}
function list(value: unknown): unknown[] | undefined {
  return Array.isArray(value) ? (value as unknown[]) : undefined;
}
const token = /gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}/;
const dbUrl = /(?:postgres(?:ql)?|mysql|mariadb|mongodb(?:\+srv)?|redis|rediss):\/\//i;
export function checkText(text: string, databaseUrls = true) {
  check(
    !token.test(text) && !(databaseUrls && dbUrl.test(text)),
    "credential/DB URL pattern found; content withheld",
  );
}
// Strict UTF-8 that keeps a leading BOM so it is refused, never skipped: RFC 8259
// JSON has none, and Bun's TOML/YAML parsers and String#trim would drop it silently.
const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
async function text(path: string) {
  const bytes = await Bun.file(path).bytes();
  let decoded: string;
  try {
    decoded = utf8.decode(bytes);
  } catch {
    throw new PolicyError("input must be UTF-8 without a BOM");
  }
  check(!decoded.startsWith("\ufeff"), "input must be UTF-8 without a BOM");
  return decoded;
}
async function json(path: string): Promise<unknown> {
  const decoded = await text(path);
  try {
    return JSON.parse(decoded);
  } catch {
    throw new PolicyError("invalid JSON input; content withheld");
  }
}
const layerCodecs = new Map<unknown, "tar" | "gzip" | "zstd">([
  ["application/vnd.oci.image.layer.v1.tar", "tar"],
  ["application/vnd.oci.image.layer.v1.tar+gzip", "gzip"],
  ["application/vnd.oci.image.layer.v1.tar+zstd", "zstd"],
  ["application/vnd.oci.image.layer.nondistributable.v1.tar", "tar"],
  ["application/vnd.oci.image.layer.nondistributable.v1.tar+gzip", "gzip"],
  ["application/vnd.oci.image.layer.nondistributable.v1.tar+zstd", "zstd"],
  ["application/vnd.docker.image.rootfs.diff.tar.gzip", "gzip"],
  ["application/vnd.docker.image.rootfs.foreign.diff.tar.gzip", "gzip"],
]);
const manifestMediaTypes: unknown[] = [
  "application/vnd.oci.image.manifest.v1+json",
  "application/vnd.docker.distribution.manifest.v2+json",
];
const configMediaTypes: unknown[] = [
  "application/vnd.oci.image.config.v1+json",
  "application/vnd.docker.container.image.v1+json",
];
const sha256Digest = /^sha256:[0-9a-f]{64}$/;
function descriptorName(descriptor: unknown) {
  const digest = at(descriptor, "digest");
  check(
    typeof digest === "string" && sha256Digest.test(digest),
    "invalid saved image descriptor digest; content withheld",
  );
  return "blobs/sha256/" + digest.slice(7);
}

const tarMessages = new Set([
  "tar: This does not look like a tar archive",
  "tar: Skipping to next header",
  "tar: Unexpected EOF in archive",
  "tar: Unexpected EOF on archive file",
  "tar: Error is not recoverable: exiting now",
  "tar: Exiting with failure status due to previous errors",
  "tar: Archive is compressed. Use -z option",
  "tar: Archive is compressed. Use --zstd option",
]);
async function tarStderrPrefix(stream: ReadableStream<Uint8Array>) {
  let prefix = "";
  let truncated = false;
  // Drain the entire pipe, but retain at most 512 bytes for diagnostics.
  const reader = stream.getReader();
  for (let next = await reader.read(); !next.done; next = await reader.read()) {
    const chunk = next.value;
    const remaining = 512 - prefix.length;
    prefix += Buffer.from(chunk.subarray(0, remaining)).toString("latin1");
    if (chunk.length > remaining) truncated = true;
  }
  // GNU tar can echo filenames and PAX values. Only fixed tool messages are safe.
  const safe = prefix
    .split("\n")
    .filter(Boolean)
    .map((line) => (tarMessages.has(line) ? line : "[content withheld]"))
    .join("; ")
    .slice(0, 512);
  return (safe || "(empty)") + (truncated ? " [truncated]" : "");
}
function layerIdentity(index: number, name: string, bytes: Uint8Array) {
  // Docker-save names may be untrusted; retain only its fixed content-addressed forms.
  const safeName = /^(?:[0-9a-f]{64}\/layer\.tar|blobs\/sha256\/[0-9a-f]{64}|layer\.tar)$/.test(
    name,
  )
    ? name
    : "name withheld";
  const magic = Buffer.from(bytes.subarray(0, 4)).toString("hex") || "empty";
  return `layer ${String(index)} (${safeName}), magic=${magic}`;
}
// Streams one archived file so memory stays near one stream chunk (Bun yields
// up to 2 MiB) plus the first MiB, which decides text vs binary (compiled
// binaries contain protocol literals). Chunks are checked in fixed 64 KiB
// windows with a 1024-character carry, so a token split at a window boundary
// is still found whatever chunk sizes the stream yields.
const textHead = 1024 * 1024;
const scanWindow = 64 * 1024;
async function scanFile(stream: ReadableStream<Uint8Array>) {
  const reader = stream.getReader();
  const head: Uint8Array[] = [];
  let buffered = 0;
  let next = await reader.read();
  for (; !next.done && buffered < textHead; next = await reader.read()) {
    head.push(next.value);
    buffered += next.value.length;
  }
  const text = !Buffer.concat(head).subarray(0, textHead).includes(0);
  let tail = "";
  const checkWindow = (bytes: Uint8Array) => {
    const window = tail + Buffer.from(bytes).toString("latin1");
    checkText(window, text); // token signatures also checked in binaries
    tail = window.slice(-1024);
  };
  // Windows start at file offsets 0, 64 KiB, ...; a shorter remainder waits for the next chunk.
  let pending: Uint8Array = new Uint8Array(0);
  const scan = (chunk: Uint8Array) => {
    const data = pending.length ? Buffer.concat([pending, chunk]) : chunk;
    let offset = 0;
    for (; data.length - offset >= scanWindow; offset += scanWindow)
      checkWindow(data.subarray(offset, offset + scanWindow));
    pending = data.subarray(offset);
  };
  for (const chunk of head) scan(chunk);
  for (; !next.done; next = await reader.read()) scan(next.value);
  if (pending.length) checkWindow(pending);
}

export function verifyWorkflow(data: unknown) {
  const on = at(data, "on");
  check(
    isObject(on) &&
      Bun.deepEquals(Object.keys(on).sort(), ["pull_request", "push", "workflow_dispatch"]),
    "unexpected image triggers",
  );
  check(Bun.deepEquals(at(on, "push", "branches"), ["main"]), "push must target main");
  for (const event of ["push", "pull_request"]) {
    const paths = list(at(on, event, "paths"));
    check(
      paths?.includes("docker/ci-base/**") &&
        paths.includes(".github/workflows/ci-base-image.yml") &&
        paths.includes("tools/ci/**") &&
        paths.includes("xtask/**") &&
        paths.includes("tests/support/official_sqld.rs"),
      "image edits must trigger builds",
    );
  }
  check(
    Bun.deepEquals(at(data, "permissions"), { contents: "read" }),
    "read-only default permissions required",
  );
  const jobs = at(data, "jobs");
  check(
    isObject(jobs) && Bun.deepEquals(Object.keys(jobs).sort(), ["build", "push", "push-manifest"]),
    "unexpected image jobs",
  );
  for (const [name, job] of Object.entries(jobs)) {
    check(
      at(job, "if") ===
        (name === "build"
          ? "github.event_name == 'pull_request'"
          : "github.ref == 'refs/heads/main'"),
      "invalid publication/build condition",
    );
    check(
      Bun.deepEquals(
        at(job, "permissions") ?? {},
        name === "build" ? {} : { contents: "read", packages: "write" },
      ),
      "invalid job token permissions",
    );
    check(
      !at(job, "container") && !at(job, "continue-on-error"),
      "image jobs must fail closed without container conversion",
    );
    if (name !== "push-manifest") {
      check(
        at(job, "runs-on") === "${{ matrix.runner }}" && at(job, "timeout-minutes") === 15,
        "native build runner/budget required",
      );
      check(
        Bun.deepEquals(at(job, "strategy", "matrix"), {
          arch: ["amd64", "arm64"],
          image: ["ci-base", "ci-web"],
          include: [
            { runner: "ubuntu-26.04", arch: "amd64" },
            { runner: "ubuntu-26.04-arm", arch: "arm64" },
          ],
        }),
        "both images/architectures must build natively",
      );
    } else {
      check(
        at(job, "needs") === "push" && at(job, "timeout-minutes") === 5,
        "manifest must follow successful pushes",
      );
    }
    const steps = list(at(job, "steps"));
    check(steps, "image job steps must be a list");
    for (const step of steps) {
      check(isObject(step), "image job steps must be mappings");
      check(!step["continue-on-error"], "steps must fail closed");
      if (step.uses)
        check(
          step.uses === "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
          "no new actions allowed",
        );
      const run = step.run ?? "";
      check(typeof run === "string", "step run must be a string");
      check(
        !JSON.stringify(step).includes("secrets.") && !run.includes("github.token"),
        "token must use github.token via env",
      );
      check(!/python3|type=gha|qemu/i.test(run), "no new Python, gha cache or QEMU");
      if (
        run.includes("docker login") ||
        run === "bun tools/ci/ci-base-image.ts login" ||
        run === "bun tools/ci/ci-base-image.ts publish-manifest"
      )
        check(
          at(step, "env", "GHCR_TOKEN") === "${{ github.token }}" &&
            (run.includes("--password-stdin") || run.startsWith("bun tools/ci/ci-base-image.ts ")),
          "token env/password-stdin required",
        );
      if (
        run.includes("docker buildx build") ||
        run === "bun tools/ci/ci-base-image.ts native-build"
      )
        check(!("if" in step), "native build step may not be skipped");
    }
    const expected = name === "push-manifest" ? "publish-manifest" : "native-build";
    check(
      steps.filter((step) => at(step, "run") === "bun tools/ci/ci-base-image.ts " + expected)
        .length === 1,
      "image execution must use the existing Bun command exactly once",
    );
  }
}

export function verifyMetadata(info: unknown, architecture: string) {
  check(
    at(info, "Architecture") === architecture && at(info, "Os") === "linux",
    "image must match native Linux architecture",
  );
  check(at(info, "Config", "User") === "1000:1000", "image default user must be 1000:1000");
  check(
    at(info, "Config", "Labels", "org.opencontainers.image.source") ===
      "https://github.com/AISFlow/fvoci",
    "OCI source label required",
  );
  const env = list(at(info, "Config", "Env") ?? []);
  check(env, "image environment must be a list");
  for (const entry of env) {
    check(typeof entry === "string", "image environment entries must be strings");
    check(
      !/TOKEN|SECRET|PASSWORD|DATABASE|DB_URL|CREDENTIAL/i.test(entry.split("=", 1)[0] ?? ""),
      "sensitive environment key found",
    );
  }
  checkText(JSON.stringify(info));
}

export async function scanImage(path: string, inspection: string, arch: string) {
  const operation = imageInterrupts();
  try {
    await scanSavedImage(path, inspection, arch, operation);
    operation.check();
  } finally {
    operation.close();
  }
}
async function scanSavedImage(
  path: string,
  inspection: string,
  arch: string,
  operation: ReturnType<typeof imageInterrupts>,
) {
  verifyMetadata(await json(inspection), arch);
  const image = new Bun.Archive(await Bun.file(path).bytes());
  async function entry(name: string, missing = "saved image entry missing") {
    const file = (await image.files(name)).get(name);
    check(file, missing);
    return file;
  }
  function parseMetadata(bytes: Uint8Array): unknown {
    try {
      return JSON.parse(utf8.decode(bytes));
    } catch {
      throw new PolicyError("invalid saved image metadata; content withheld");
    }
  }
  async function descriptorBytes(descriptor: unknown) {
    const bytes = await (await entry(descriptorName(descriptor))).bytes();
    const size = at(descriptor, "size");
    check(
      typeof size === "number" &&
        Number.isSafeInteger(size) &&
        size >= 0 &&
        size === bytes.length &&
        at(descriptor, "digest") ===
          "sha256:" + new Bun.CryptoHasher("sha256").update(bytes).digest("hex"),
      "saved image descriptor content mismatch; content withheld",
    );
    return bytes;
  }
  const manifests = list(parseMetadata(await (await entry("manifest.json")).bytes()));
  const legacy = manifests?.length === 1 ? manifests[0] : undefined;
  const legacyLayers = list(at(legacy, "Layers"));
  check(legacyLayers && legacyLayers.length > 0, "expected one saved image with layers");
  const layout = parseMetadata(
    await (
      await entry("oci-layout", "saved image OCI descriptors missing; content withheld")
    ).bytes(),
  );
  check(
    at(layout, "imageLayoutVersion") === "1.0.0",
    "unsupported saved image layout; content withheld",
  );
  const index = parseMetadata(
    await (
      await entry("index.json", "saved image OCI descriptors missing; content withheld")
    ).bytes(),
  );
  const indexManifests = list(at(index, "manifests"));
  check(
    at(index, "schemaVersion") === 2 &&
      at(index, "mediaType") === "application/vnd.oci.image.index.v1+json" &&
      indexManifests?.length === 1,
    "expected one saved image OCI manifest; content withheld",
  );
  const manifestDescriptor = indexManifests[0];
  check(
    manifestMediaTypes.includes(at(manifestDescriptor, "mediaType")),
    "unsupported saved image manifest media type; content withheld",
  );
  const manifest = parseMetadata(await descriptorBytes(manifestDescriptor));
  const layers = list(at(manifest, "layers"));
  check(
    at(manifest, "schemaVersion") === 2 &&
      at(manifest, "mediaType") === at(manifestDescriptor, "mediaType") &&
      layers &&
      layers.length > 0 &&
      configMediaTypes.includes(at(manifest, "config", "mediaType")),
    "invalid saved image OCI manifest; content withheld",
  );
  check(
    at(legacy, "Config") === descriptorName(at(manifest, "config")) &&
      Bun.deepEquals(legacyLayers, layers.map(descriptorName)),
    "saved image OCI/legacy references disagree; content withheld",
  );
  const config = await descriptorBytes(at(manifest, "config"));
  parseMetadata(config);
  checkText(utf8.decode(config));
  // Include lower layers: deleting a credential later does not remove it.
  for (const [index, descriptor] of layers.entries()) {
    operation.check();
    const layer = descriptorName(descriptor);
    let bytes: Uint8Array = await descriptorBytes(descriptor);
    const identity = layerIdentity(index, layer, bytes);
    const codec = layerCodecs.get(at(descriptor, "mediaType"));
    check(codec, `unsupported layer media type: ${identity}; content withheld`);
    try {
      // Decode once so GNU tar and Bun inspect the same uncompressed archive.
      if (codec === "gzip") {
        const decoded = gunzipSync(bytes, { info: true }) as unknown as {
          buffer: Uint8Array;
          engine: { bytesWritten: number };
        };
        // Gunzip can stop at NUL padding and silently discard the remaining input.
        check(decoded.engine.bytesWritten === bytes.length, "unconsumed gzip input");
        bytes = decoded.buffer;
      } else if (codec === "zstd") {
        bytes = await Bun.zstdDecompress(bytes);
      }
    } catch {
      throw new PolicyError(`layer decompression failed: ${identity}; content withheld`);
    }
    // Include entries after zero blocks/concatenated archives, including non-regulars.
    operation.check();
    const listing = Bun.spawn(["tar", "--ignore-zeros", "-tf", "-"], {
      stdin: bytes,
      stdout: "pipe",
      stderr: "pipe",
      env: { ...process.env, LC_ALL: "C", TAR_OPTIONS: "" },
    });
    const stop = () => listing.kill("SIGTERM");
    operation.signal.addEventListener("abort", stop, { once: true });
    const reads = [
      new Response(listing.stdout).text(),
      tarStderrPrefix(listing.stderr),
      listing.exited,
    ] as const;
    let names: string, stderr: string, exitCode: number;
    try {
      operation.check();
      [names, stderr, exitCode] = await Promise.all(reads);
      operation.check();
    } finally {
      operation.signal.removeEventListener("abort", stop);
      if (listing.exitCode === null) listing.kill("SIGTERM");
      await Promise.allSettled(reads);
    }
    // GNU tar can ignore a final partial block; reject that framing without parsing headers.
    const framing = bytes.length % 512 === 0 ? "" : "incomplete tar block; ";
    check(
      exitCode === 0 && !framing,
      `layer archive listing failed: ${identity}; ${framing}stderr prefix: ${stderr}`,
    );
    check(!/(^|\/)\.env(?:[.\s]|$)/m.test(names), "environment file found; content withheld");
    // No consumer of these images runs Python; a dependency that pulls in an
    // interpreter or its standard library must fail the build.
    check(
      !/^(?:\.\/)?(?:usr\/)?(?:local\/)?(?:bin\/python[0-9.]*$|lib\/python[0-9]+\.[0-9]+\/)/m.test(
        names,
      ),
      `Python interpreter found: ${identity}`,
    );
    const files = await new Bun.Archive(bytes).files();
    for (const [name, file] of files) {
      operation.check();
      check(
        basename(name) !== ".env" && !basename(name).startsWith(".env."),
        "environment file found; content withheld",
      );
      await scanFile(file.stream());
    }
  }
  console.log("Image metadata and all saved layers: credential/env/DB URL checks passed");
}

export function buildSummary(log: string, info?: unknown) {
  const rows = ["\n| Build step | Seconds |", "| --- | ---: |"];
  const descriptions = new Map<string, string>();
  for (const line of log.split("\n")) {
    const description = /^(#\d+) (\[.*?\] .*)/.exec(line);
    if (description?.[1] && description[2])
      descriptions.set(description[1], description[2].replaceAll("|", "\\|"));
    const done = /^(#\d+) (?:DONE ([0-9.]+)s|(CACHED))$/.exec(line);
    const step = done?.[1] === undefined ? undefined : descriptions.get(done[1]);
    if (done && step !== undefined) rows.push(`| ${step} | ${done[2] ?? "cached"} |`);
  }
  // Reporting only: an inspection without these fields omits the size line.
  const size = at(info, "Size");
  const architecture = at(info, "Architecture");
  if (typeof size === "number" && typeof architecture === "string")
    rows.push(`\nUncompressed image size: ${String(size)} bytes (${architecture}).`);
  return rows.join("\n");
}

export function archDigest(config: unknown, manifest: unknown, arch: string) {
  check(
    at(config, "architecture") === arch &&
      at(config, "os") === "linux" &&
      at(config, "config", "User") === "1000:1000",
    "published architecture/user mismatch",
  );
  const digest = at(manifest, "digest");
  check(typeof digest === "string" && sha256Digest.test(digest), "invalid architecture digest");
  return digest;
}
export function verifyManifest(index: unknown, sources: string[]) {
  const manifests = list(at(index, "manifests"));
  const digest = at(index, "digest");
  check(
    manifests?.length === 2 && typeof digest === "string" && sha256Digest.test(digest),
    "invalid multi-arch manifest",
  );
  check(
    Bun.deepEquals(
      manifests
        .map(
          (m) => `${String(at(m, "platform", "os"))}/${String(at(m, "platform", "architecture"))}`,
        )
        .sort(),
      ["linux/amd64", "linux/arm64"],
    ),
    "wrong manifest platforms",
  );
  const digests = manifests.map((m) => at(m, "digest"));
  check(
    digests.every((value) => typeof value === "string" && sha256Digest.test(value)) &&
      Bun.deepEquals(digests.sort(), sources.map((s) => s.split("@")[1]).sort()),
    "manifest must use verified source digests",
  );
  return digest;
}

function operand(args: readonly string[], index: number) {
  const value = args[index];
  check(value !== undefined, "missing image check argument");
  return value;
}

if (import.meta.main) {
  const [command, ...args] = process.argv.slice(2);
  try {
    switch (command) {
      case "inputs": {
        const rust = Bun.TOML.parse(await text("rust-toolchain.toml"));
        check(
          at(rust, "toolchain", "channel") === "1.99.0" &&
            (await text(".bun-version")).trim() === "1.4.3",
          "tool versions differ from recipe",
        );
        check(
          at(await json("apps/web/package.json"), "devDependencies", "@playwright/test") ===
            "1.64.0",
          "Playwright pin differs from recipe",
        );
        verifyWorkflow(Bun.YAML.parse(await text(".github/workflows/ci-base-image.yml")));
        break;
      }
      case "login":
        await imageLogin();
        break;
      case "native-build":
        await nativeImageBuild();
        break;
      case "publish-manifest":
        await publishImageManifest();
        break;
      case "smoke":
        await smokeImage(operand(args, 0), operand(args, 1), operand(args, 2));
        break;
      case "scan":
        await scanImage(operand(args, 0), operand(args, 1), operand(args, 2));
        break;
      case "summary": {
        const inspection = operand(args, 1);
        console.log(
          buildSummary(
            await Bun.file(operand(args, 0)).text(),
            (await Bun.file(inspection).exists()) ? await json(inspection) : undefined,
          ),
        );
        break;
      }
      case "arch-digest":
        console.log(
          archDigest(await json(operand(args, 0)), await json(operand(args, 1)), operand(args, 2)),
        );
        break;
      case "layer-size": {
        const layers = list(at(await json(operand(args, 0)), "layers"));
        check(layers, "invalid layer manifest");
        let size = 0;
        for (const layer of layers) {
          const layerSize = at(layer, "size");
          check(
            typeof layerSize === "number" && Number.isSafeInteger(layerSize) && layerSize >= 0,
            "invalid layer size",
          );
          size += layerSize;
        }
        console.log(`Compressed layer size (${operand(args, 1)}): ${String(size)} bytes`);
        break;
      }
      case "manifest": {
        const index = await json(operand(args, 0));
        const image = operand(args, 1);
        console.log(`Final multi-arch image: ${image}@${verifyManifest(index, args.slice(2))}`);
        break;
      }
      default:
        throw new PolicyError("unknown image check command");
    }
  } catch (error) {
    console.error(
      "ci-base check failed:",
      error instanceof PolicyError ? error.message : "archive/tool failure; content withheld",
    );
    process.exitCode ||= 1;
  }
}

function imageEnv(name: string): string {
  const value = process.env[name];
  check(value && !/[\r\n\0]/.test(value), "missing/invalid image execution input");
  return value;
}
export function imageBuildPlan(env: Record<string, string | undefined>, architecture: string) {
  const arch = env.ARCH,
    target = env.TARGET;
  check(
    (arch === "amd64" && architecture === "x64") || (arch === "arm64" && architecture === "arm64"),
    "native image architecture required",
  );
  check(target === "ci-base" || target === "ci-web", "unknown CI image target");
  const source = env.GITHUB_SHA,
    run = env.GITHUB_RUN_ID,
    attempt = env.GITHUB_RUN_ATTEMPT;
  check(
    source &&
      /^[0-9a-f]{40}$/.test(source) &&
      run &&
      /^[1-9][0-9]*$/.test(run) &&
      attempt &&
      /^[1-9][0-9]*$/.test(attempt),
    "exact source/run/attempt required",
  );
  check(
    env.PUBLISH === "true" || env.PUBLISH === "false",
    "explicit image publication selection required",
  );
  check(
    env.PUBLISH !== "true" || env.GITHUB_REF === "refs/heads/main",
    "publication requires main",
  );
  const image = `ghcr.io/aisflow/fvoci-${target}`;
  return {
    arch,
    target,
    image,
    builder: `${target}-${run}-${attempt}-${arch}`,
    ref: `${image}:${source}-${run}-${attempt}-${arch}`,
    args: [
      "--platform",
      `linux/${arch}`,
      "--file",
      "docker/ci-base/Dockerfile",
      "--target",
      target,
      "--label",
      "org.opencontainers.image.source=https://github.com/AISFlow/fvoci",
      "--label",
      `org.opencontainers.image.revision=${source}`,
      "--provenance=false",
      "--progress=plain",
    ],
  };
}

// Image-only argv execution. Signals terminate and await the owned CLI child;
// nativeImageBuild then removes its owned BuildKit worker in finally.
export async function imageCommand(
  argv: string[],
  options: { log?: string; input?: string } = {},
): Promise<string> {
  const environment = { ...process.env };
  delete environment.GHCR_TOKEN;
  delete environment.GITHUB_TOKEN;
  const stage = imageStage(argv);
  let interrupted = false;
  const child = (() => {
    try {
      return Bun.spawn(argv, {
        env: environment,
        stdin: options.input === undefined ? "ignore" : "pipe",
        stdout: "pipe",
        stderr: "pipe",
      });
    } catch (cause) {
      throw new PolicyError("image stage=" + stage + " spawn failed", { cause });
    }
  })();
  const interrupt = (code: number) => {
    interrupted = true;
    process.exitCode = code;
    child.kill("SIGTERM");
  };
  const onInt = () => interrupt(130),
    onTerm = () => interrupt(143);
  process.once("SIGINT", onInt);
  process.once("SIGTERM", onTerm);
  let output = "",
    bytes = 0;
  async function drain(stream: ReadableStream<Uint8Array>, stdout: boolean) {
    for await (const chunk of stream) {
      bytes += chunk.length;
      if (bytes > (options.log ? 128 * 1024 * 1024 : 16 * 1024 * 1024)) {
        child.kill("SIGTERM");
        throw new PolicyError("image command output bound exceeded");
      }
      if (options.log) {
        appendFileSync(options.log, chunk);
        process.stdout.write(chunk);
      } else if (stdout) output += new TextDecoder().decode(chunk);
    }
  }
  try {
    const input = child.stdin;
    if (
      options.input !== undefined &&
      input !== undefined &&
      input !== null &&
      typeof input !== "number"
    ) {
      await input.write(options.input);
      await input.end();
    }
    const drained = await Promise.allSettled([
      drain(child.stdout, true),
      drain(child.stderr, false),
    ]);
    const code = await child.exited;
    check(
      !interrupted &&
        drained.every((result) => result.status === "fulfilled") &&
        code === 0 &&
        !child.signalCode,
      `image stage=${stage} exit=${String(code)} signal=${child.signalCode ?? "none"} interrupted=${String(interrupted)} failed`,
    );
    return output.trim();
  } catch (cause) {
    if (cause instanceof PolicyError) throw cause;
    throw new PolicyError(`image stage=${stage} I/O failed interrupted=${String(interrupted)}`, {
      cause,
    });
  } finally {
    process.off("SIGINT", onInt);
    process.off("SIGTERM", onTerm);
    if (child.exitCode === null) child.kill("SIGTERM");
    await child.exited;
  }
}
async function imageLogin() {
  check(imageEnv("GITHUB_REF") === "refs/heads/main", "publication login requires main");
  const path = imageEnv("DOCKER_CONFIG");
  check(
    path === join(imageEnv("RUNNER_TEMP"), "ci-base-docker") ||
      path === join(imageEnv("RUNNER_TEMP"), "ci-base-manifest-docker"),
    "owned image credential directory required",
  );
  mkdirSync(path, { recursive: true, mode: 0o700 });
  await imageCommand(
    ["docker", "login", "ghcr.io", "-u", imageEnv("GITHUB_ACTOR"), "--password-stdin"],
    { input: imageEnv("GHCR_TOKEN") },
  );
}

// Runs inside the final tool image under its non-root user and read-only root
// filesystem. Only explicit temporary/cache mounts are writable.
export function imageSmokeScript(): string {
  return String.raw`
const fs = require("node:fs"), path = require("node:path");
const check = (value) => { if (!value) throw new Error("image tool/SDK smoke refused"); };
const run = (args) => { const child = Bun.spawnSync(args, {stdout:"pipe",stderr:"pipe"}); check(child.exitCode === 0 && !child.signalCode); return child.stdout.toString().trim(); };
check(process.getuid() === 1000 && process.getgid() === 1000);
check(run(["rustc","--version"]).split(" ")[1] === "1.99.0");
check(run(["bun","--version"]) === "1.4.3");
function immutable(root) { const stat=fs.statSync(root); check(stat.uid===0 && (stat.mode & 0o022)===0); try {fs.accessSync(root,fs.constants.W_OK); throw new Error("writable tool");} catch(error) {check(error.code === "EROFS" || error.code === "EACCES");} if(stat.isDirectory()) for(const name of fs.readdirSync(root)) immutable(path.join(root,name)); }
for(const root of ["/opt/rustup","/opt/cargo","/opt/fvoci-sqlite","/usr/local/bin/fvoci-ci-tools"]) immutable(root);
for(const name of ["registry","git"]) { const directory=path.join(process.env.CARGO_HOME,name); fs.mkdirSync(directory,{recursive:true}); fs.accessSync(directory,fs.constants.W_OK); }
for(const command of [["cargo","--version"],["cargo","fmt","--version"],["cargo","clippy","--version"],["git","--version"],["zstd","--version"],["dpkg-query","-s","ca-certificates"]]) run(command);
check(!fs.existsSync("/tmp/bootstrap-ca.crt"));
for(const line of fs.readFileSync("/var/lib/dpkg/info/ca-certificates.md5sums","utf8").trim().split("\n")) { const match=/^([a-f0-9]{32})\s+(.+)$/.exec(line); check(match); if(/^usr\/share\/(doc|man)\//.test(match[2])) continue; check(new Bun.CryptoHasher("md5").update(fs.readFileSync("/"+match[2])).digest("hex")===match[1]); }
check(fs.readdirSync("/usr/local/share/ca-certificates").length===0);
check(fs.statSync("/etc/ssl/certs/ca-certificates.crt").size>0 && fs.existsSync(path.join(process.env.LIBCLANG_PATH,"libclang.so")));
const parent=fs.mkdtempSync("/tmp/fvoci-image-sdk-");
try { const identity=new Bun.CryptoHasher("sha256").update(fs.readFileSync("/opt/fvoci-sqlite/consumer-inputs.json")).digest("hex");
 run(["/usr/local/bin/fvoci-ci-tools","sqlite-ci","--local-image-sdk","--image-sdk","/opt/fvoci-sqlite","--expected-sdk-sha256",identity,"--image-ref",process.env.FVOCI_SDK_IMAGE_REF,"--parent",parent]);
 if(process.env.FVOCI_IMAGE_TARGET === "ci-web") { function browser(root) { for(const name of fs.readdirSync(root)){const p=path.join(root,name),s=fs.statSync(p);if(s.isDirectory()){const b=browser(p);if(b)return b;} else if(name==="chrome" && /chrome-linux/.test(p))return p;} } const executable=browser(process.env.PLAYWRIGHT_BROWSERS_PATH);check(executable);check(run([executable,"--headless","--no-sandbox","--disable-gpu","--user-data-dir="+path.join(parent,"chromium"),"--dump-dom","about:blank"]).includes("<html")); }
} finally { fs.rmSync(parent,{recursive:true,force:true}); }
`;
}
export async function smokeImage(ref: string, target: string, name: string) {
  check(
    sha256Digest.test(ref) && (target === "ci-base" || target === "ci-web"),
    "smoke requires exact local image digest and target",
  );
  ownedImageName(name);
  check(
    (await namedContainer(name)) === null,
    "image smoke name already exists; refusing adoption",
  );
  let failure: unknown;
  try {
    await imageCommand([
      "docker",
      "run",
      "--rm",
      "--name",
      name,
      "--label",
      "org.fvoci.ci.smoke-owner=" + name,
      "--network",
      "none",
      "--read-only",
      "--cpus",
      "2",
      "--memory",
      "16g",
      "--tmpfs",
      "/tmp:rw,mode=1777",
      "--tmpfs",
      "/home/ci/.cargo:rw,uid=1000,gid=1000,mode=0755",
      "--env",
      `FVOCI_SDK_IMAGE_REF=${ref}`,
      "--env",
      `FVOCI_IMAGE_TARGET=${target}`,
      ref,
      "bun",
      "-e",
      imageSmokeScript(),
    ]);
  } catch (error) {
    failure = error;
  }
  const cleanupErrors: unknown[] = [];
  try {
    const container = await namedContainer(name);
    if (container !== null) {
      check(
        at(container, "Name") === "/" + name &&
          at(container, "Image") === ref &&
          at(container, "Config", "Labels", "org.fvoci.ci.smoke-owner") === name,
        "refusing to retire an unowned image smoke container",
      );
      await imageCommand(["docker", "container", "rm", "--force", name]);
      if (!failure)
        cleanupErrors.push(new PolicyError("image smoke container outlived successful CLI"));
    }
    check((await namedContainer(name)) === null, "image smoke container retirement unconfirmed");
  } catch (error) {
    cleanupErrors.push(error);
  }
  imageOutcome("smoke", failure, cleanupErrors);
}
async function nativeImageBuild() {
  const plan = imageBuildPlan(process.env, process.arch);
  const dockerConfig = imageEnv("DOCKER_CONFIG");
  const runnerTemp = imageEnv("RUNNER_TEMP");
  const work = mkdtempSync(join(runnerTemp, "ci-base-"));
  const operation = imageInterrupts();
  const run: typeof imageCommand = (argv, options) => {
    operation.check();
    return imageCommand(argv, options);
  };
  const log = join(work, "build.log"),
    inspection = join(work, "image.json");
  let builderOwned = false,
    failure: unknown;
  const cleanupErrors: unknown[] = [];
  const summary = (line: string) => appendFileSync(imageEnv("GITHUB_STEP_SUMMARY"), line + "\n");
  try {
    await run([process.execPath, "tools/ci/ci-base-image.ts", "inputs"]);
    await run([process.execPath, "test", "tools/ci/ci-base-image.test.ts"]);
    await run(["docker", "buildx", "version"]);
    operation.check();
    await allocateImageBuilder(plan.builder);
    builderOwned = true;
    await run(["docker", "buildx", "inspect", "--bootstrap"]);
    const started = performance.now();
    await run(
      [
        "docker",
        "buildx",
        "build",
        ...plan.args,
        "--tag",
        plan.ref,
        "--load",
        "--cache-from",
        `type=registry,ref=${plan.image}:cache-${plan.arch}`,
        ".",
      ],
      { log },
    );
    summary(
      `Native build ${plan.target}/${plan.arch}: ${String((performance.now() - started) / 1000)}s`,
    );
    const metadata: unknown = JSON.parse(
      await run(["docker", "image", "inspect", plan.ref, "--format", "{{json .}}"]),
    );
    await Bun.write(inspection, JSON.stringify(metadata));
    await run(["docker", "image", "save", plan.ref, "--output", join(work, "image.tar")]);
    await scanImage(join(work, "image.tar"), inspection, plan.arch);
    operation.check();
    const id = at(metadata, "Id");
    check(typeof id === "string" && sha256Digest.test(id), "exact inspected image ID required");
    await smokeImage(id, plan.target, plan.builder + "-smoke");
    operation.check();
    if (process.env.PUBLISH === "true") {
      const started = performance.now();
      await run(["docker", "push", plan.ref]);
      await run([
        "docker",
        "buildx",
        "build",
        ...plan.args,
        "--output",
        "type=cacheonly",
        "--cache-to",
        `type=registry,ref=${plan.image}:cache-${plan.arch},mode=max`,
        ".",
      ]);
      summary(
        `Image/cache publish ${plan.target}/${plan.arch}: ${String((performance.now() - started) / 1000)}s`,
      );
      summary(await run(["docker", "buildx", "imagetools", "inspect", plan.ref]));
    }
  } catch (error) {
    failure = error;
  } finally {
    try {
      if (existsSync(log))
        summary(
          buildSummary(
            readFileSync(log, "utf8"),
            existsSync(inspection) ? JSON.parse(readFileSync(inspection, "utf8")) : undefined,
          ),
        );
    } catch (error) {
      cleanupErrors.push(error);
    }
    if (builderOwned)
      try {
        await retireImageBuilder(plan.builder);
      } catch (error) {
        cleanupErrors.push(error);
      }
    try {
      await imageCommand(["docker", "logout", "ghcr.io"]);
    } catch (error) {
      cleanupErrors.push(error);
    }
    for (const path of [work, dockerConfig, join(runnerTemp, "ci-base-bun")])
      try {
        rmSync(path, { recursive: true, force: true });
      } catch (error) {
        cleanupErrors.push(error);
      }
    operation.close();
  }
  imageOutcome("build", failure, cleanupErrors);
}
async function publishImageManifest() {
  check(imageEnv("GITHUB_REF") === "refs/heads/main", "manifest publication requires main");
  const plan = imageBuildPlan({ ...process.env, ARCH: "amd64", PUBLISH: "true" }, "x64");
  const work = mkdtempSync(join(imageEnv("RUNNER_TEMP"), "ci-base-manifest-"));
  let failure: unknown;
  const cleanupErrors: unknown[] = [];
  const summary = (line: string) => appendFileSync(imageEnv("GITHUB_STEP_SUMMARY"), line + "\n");
  try {
    await imageLogin();
    const sources: string[] = [];
    for (const arch of ["amd64", "arm64"]) {
      const ref = plan.ref.replace(/amd64$/, arch);
      const config: unknown = JSON.parse(
        await imageCommand([
          "docker",
          "buildx",
          "imagetools",
          "inspect",
          ref,
          "--format",
          "{{json .Image}}",
        ]),
      );
      const manifest: unknown = JSON.parse(
        await imageCommand([
          "docker",
          "buildx",
          "imagetools",
          "inspect",
          ref,
          "--format",
          "{{json .Manifest}}",
        ]),
      );
      sources.push(`${plan.image}@${archDigest(config, manifest, arch)}`);
      const raw: unknown = JSON.parse(
        await imageCommand(["docker", "buildx", "imagetools", "inspect", ref, "--raw"]),
      );
      const layers = list(at(raw, "layers"));
      check(layers, "invalid image layers");
      let size = 0;
      for (const layer of layers) {
        const bytes = at(layer, "size");
        check(
          typeof bytes === "number" && Number.isSafeInteger(bytes) && bytes >= 0,
          "invalid layer size",
        );
        size += bytes;
      }
      summary(`Compressed layer size (${arch}): ${String(size)} bytes`);
    }
    const started = performance.now(),
      ref = `${plan.image}:sha-${imageEnv("GITHUB_SHA")}`;
    await imageCommand(["docker", "buildx", "imagetools", "create", "--tag", ref, ...sources]);
    const index: unknown = JSON.parse(
      await imageCommand([
        "docker",
        "buildx",
        "imagetools",
        "inspect",
        ref,
        "--format",
        "{{json .Manifest}}",
      ]),
    );
    summary(`Multi-arch reference: ${plan.image}@${String(verifyManifest(index, sources))}`);
    summary(`Manifest publish: ${String((performance.now() - started) / 1000)}s`);
  } catch (error) {
    failure = error;
  } finally {
    try {
      await imageCommand(["docker", "logout", "ghcr.io"]);
    } catch (error) {
      cleanupErrors.push(error);
    }
    for (const path of [
      work,
      imageEnv("DOCKER_CONFIG"),
      join(imageEnv("RUNNER_TEMP"), "ci-base-bun"),
    ])
      try {
        rmSync(path, { recursive: true, force: true });
      } catch (error) {
        cleanupErrors.push(error);
      }
  }
  imageOutcome("manifest", failure, cleanupErrors);
}

function imageStage(argv: string[]): string {
  if (argv[0] !== "docker")
    return argv[1] === "test"
      ? "image-unit"
      : argv[2] === "inputs"
        ? "image-inputs"
        : "image-child";
  const known = new Set([
    "buildx",
    "create",
    "inspect",
    "version",
    "build",
    "rm",
    "imagetools",
    "image",
    "save",
    "container",
    "ls",
    "run",
    "login",
    "logout",
    "push",
  ]);
  return ["docker", ...argv.slice(1, 4).filter((part) => known.has(part))].join("-");
}
function imageOutcome(
  stage: "build" | "manifest" | "smoke" | "builder-allocation" | "builder-retirement",
  failure: unknown,
  cleanup: unknown[],
): void {
  if (!failure && cleanup.length === 0) return;
  const safe = (error: unknown): string =>
    error instanceof PolicyError
      ? error.message
      : error instanceof AggregateError
        ? error.errors.map(safe).join("; ")
        : "image operation failed; details withheld";
  const cause = new AggregateError(
    [...(failure ? [failure] : []), ...cleanup],
    `image ${stage} first=${failure ? safe(failure) : "none"}; cleanup=${cleanup.map(safe).join("; ") || "none"}`,
  );
  throw new PolicyError(cause.message, { cause });
}
function ownedImageName(name: string): void {
  check(
    /^[a-z0-9][a-z0-9-]{0,120}$/.test(name),
    "deterministic run-owned image resource name required",
  );
}
async function namedContainer(name: string): Promise<unknown | null> {
  const ids = await imageCommand([
    "docker",
    "container",
    "ls",
    "--all",
    "--filter",
    `name=^/${name}$`,
    "--format",
    "{{.ID}}",
  ]);
  if (!ids) return null;
  check(/^[a-f0-9]{12,64}$/.test(ids), "image resource lookup is ambiguous");
  return JSON.parse(
    await imageCommand(["docker", "container", "inspect", name, "--format", "{{json .}}"]),
  );
}
async function builderPresent(name: string): Promise<boolean> {
  const names = await imageCommand(["docker", "buildx", "ls", "--format", "{{.Name}}"]);
  return names.split("\n").includes(name);
}
export async function allocateImageBuilder(name: string): Promise<void> {
  ownedImageName(name);
  check(
    !(await builderPresent(name)) && (await namedContainer(`buildx_buildkit_${name}0`)) === null,
    "image builder name already exists; refusing adoption",
  );
  // The positively absent name is claimed before create can partially allocate.
  try {
    await imageCommand([
      "docker",
      "buildx",
      "create",
      "--name",
      name,
      "--driver",
      "docker-container",
      "--use",
      "--driver-opt",
      "memory=16g",
      "--driver-opt",
      "cpu-period=100000",
      "--driver-opt",
      "cpu-quota=200000",
      "--driver-opt",
      `env.FVOCI_CI_BUILDER_OWNER=${name}`,
    ]);
  } catch (failure) {
    const cleanup: unknown[] = [];
    try {
      await retireImageBuilder(name);
    } catch (error) {
      cleanup.push(error);
    }
    imageOutcome("builder-allocation", failure, cleanup);
  }
}
export async function retireImageBuilder(name: string): Promise<void> {
  ownedImageName(name);
  const cleanup: unknown[] = [];
  try {
    if (await builderPresent(name)) await imageCommand(["docker", "buildx", "rm", name]);
  } catch (error) {
    cleanup.push(error);
  }
  const worker = `buildx_buildkit_${name}0`;
  try {
    const container = await namedContainer(worker);
    if (container !== null) {
      const env = list(at(container, "Config", "Env"));
      check(
        at(container, "Name") === "/" + worker && env?.includes(`FVOCI_CI_BUILDER_OWNER=${name}`),
        "refusing to retire an unowned BuildKit worker",
      );
      await imageCommand(["docker", "container", "rm", "--force", worker]);
    }
    check(
      !(await builderPresent(name)) && (await namedContainer(worker)) === null,
      "image builder retirement unconfirmed",
    );
  } catch (error) {
    cleanup.push(error);
  }
  imageOutcome("builder-retirement", undefined, cleanup);
}
