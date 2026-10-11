// CI-only policy around Bun's YAML/TOML/tar implementations and GNU tar.
// No extraction to disk and no matched credential values are printed.
import { basename } from "node:path";
import { gunzipSync } from "node:zlib";

class PolicyError extends Error {}
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
        paths.includes("tools/ci/**"),
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
          step.uses === "actions/checkout@11d5960a326750d5838078e36cf38b85af677262",
          "no new actions allowed",
        );
      const run = step.run ?? "";
      check(typeof run === "string", "step run must be a string");
      check(
        !JSON.stringify(step).includes("secrets.") && !run.includes("github.token"),
        "token must use github.token via env",
      );
      check(!/python3|type=gha|qemu/i.test(run), "no new Python, gha cache or QEMU");
      if (run.includes("docker login"))
        check(
          at(step, "env", "GHCR_TOKEN") === "${{ github.token }}" &&
            run.includes("--password-stdin"),
          "token env/password-stdin required",
        );
      if (run.includes("docker buildx build"))
        check(!("if" in step), "native build step may not be skipped");
    }
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
    const listing = Bun.spawn(["tar", "--ignore-zeros", "-tf", "-"], {
      stdin: bytes,
      stdout: "pipe",
      stderr: "pipe",
      env: { ...process.env, LC_ALL: "C", TAR_OPTIONS: "" },
    });
    const [names, stderr, exitCode] = await Promise.all([
      new Response(listing.stdout).text(),
      tarStderrPrefix(listing.stderr),
      listing.exited,
    ]);
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
          at(rust, "toolchain", "channel") === "1.98.1" &&
            (await text(".bun-version")).trim() === "1.4.2",
          "tool versions differ from recipe",
        );
        check(
          at(await json("apps/web/package.json"), "devDependencies", "@playwright/test") ===
            "1.63.0",
          "Playwright pin differs from recipe",
        );
        verifyWorkflow(Bun.YAML.parse(await text(".github/workflows/ci-base-image.yml")));
        break;
      }
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
    process.exitCode = 1;
  }
}
