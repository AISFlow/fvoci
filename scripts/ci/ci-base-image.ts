// CI-only policy around Bun's YAML/TOML/tar implementations and GNU tar.
// No extraction to disk and no matched credential values are printed.
import { basename } from "node:path";
import { gunzipSync } from "node:zlib";

class PolicyError extends Error {}
function check(condition: unknown, message: string): asserts condition {
  if (!condition) throw new PolicyError(message);
}
const token = /gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}/;
const dbUrl = /(?:postgres(?:ql)?|mysql|mariadb|mongodb(?:\+srv)?|redis|rediss):\/\//i;
export function checkText(text: string, databaseUrls = true) {
  check(
    !token.test(text) && !(databaseUrls && dbUrl.test(text)),
    "credential/DB URL pattern found; content withheld",
  );
}
const json = async (path: string) => JSON.parse(await Bun.file(path).text());

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
  for await (const chunk of stream) {
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
  return `layer ${index} (${safeName}), magic=${magic}`;
}

export function verifyWorkflow(data: any) {
  check(
    Bun.deepEquals(Object.keys(data.on).sort(), ["pull_request", "push", "workflow_dispatch"]),
    "unexpected image triggers",
  );
  check(Bun.deepEquals(data.on.push.branches, ["main"]), "push must target main");
  for (const event of [data.on.push, data.on.pull_request]) {
    check(
      event.paths.includes("docker/ci-base/**") &&
        event.paths.includes(".github/workflows/ci-base-image.yml") &&
        event.paths.includes("scripts/ci/**"),
      "image edits must trigger builds",
    );
  }
  check(
    Bun.deepEquals(data.permissions, { contents: "read" }),
    "read-only default permissions required",
  );
  check(
    Bun.deepEquals(Object.keys(data.jobs).sort(), ["build", "push", "push-manifest"]),
    "unexpected image jobs",
  );
  for (const [name, job] of Object.entries(data.jobs) as [string, any][]) {
    check(
      job.if ===
        (name === "build"
          ? "github.event_name == 'pull_request'"
          : "github.ref == 'refs/heads/main'"),
      "invalid publication/build condition",
    );
    check(
      Bun.deepEquals(
        job.permissions ?? {},
        name === "build" ? {} : { contents: "read", packages: "write" },
      ),
      "invalid job token permissions",
    );
    check(
      !job.container && !job["continue-on-error"],
      "image jobs must fail closed without container conversion",
    );
    if (name !== "push-manifest") {
      check(
        job["runs-on"] === "${{ matrix.runner }}" && job["timeout-minutes"] === 15,
        "native build runner/budget required",
      );
      check(
        Bun.deepEquals(job.strategy.matrix, {
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
        job.needs === "push" && job["timeout-minutes"] === 5,
        "manifest must follow successful pushes",
      );
    }
    for (const step of job.steps) {
      check(!step["continue-on-error"], "steps must fail closed");
      if (step.uses)
        check(
          step.uses === "actions/checkout@11d5960a326750d5838078e36cf38b85af677262",
          "no new actions allowed",
        );
      const run = step.run ?? "";
      check(
        !JSON.stringify(step).includes("secrets.") && !run.includes("github.token"),
        "token must use github.token via env",
      );
      check(!/python3|type=gha|qemu/i.test(run), "no new Python, gha cache or QEMU");
      if (run.includes("docker login"))
        check(
          step.env.GHCR_TOKEN === "${{ github.token }}" && run.includes("--password-stdin"),
          "token env/password-stdin required",
        );
      if (run.includes("docker buildx build"))
        check(!("if" in step), "native build step may not be skipped");
    }
  }
}

export function verifyMetadata(info: any, architecture: string) {
  check(
    info.Architecture === architecture && info.Os === "linux",
    "image must match native Linux architecture",
  );
  check(info.Config.User === "1000:1000", "image default user must be 1000:1000");
  check(
    info.Config.Labels?.["org.opencontainers.image.source"] === "https://github.com/AISFlow/fvoci",
    "OCI source label required",
  );
  for (const env of info.Config.Env ?? [])
    check(
      !/TOKEN|SECRET|PASSWORD|DATABASE|DB_URL|CREDENTIAL/i.test(env.split("=", 1)[0]),
      "sensitive environment key found",
    );
  checkText(JSON.stringify(info));
}

export async function scanImage(path: string, inspection: string, arch: string) {
  verifyMetadata(await json(inspection), arch);
  const image = new Bun.Archive(await Bun.file(path).bytes());
  async function entry(name: string) {
    const file = (await image.files(name)).get(name);
    check(file, "saved image entry missing");
    return file;
  }
  const manifests = JSON.parse(await (await entry("manifest.json")).text());
  check(
    manifests.length === 1 && manifests[0].Layers.length > 0,
    "expected one saved image with layers",
  );
  checkText(await (await entry(manifests[0].Config)).text());
  // Include lower layers: deleting a credential later does not remove it.
  for (const [index, layer] of manifests[0].Layers.entries()) {
    const blob = await entry(layer);
    let bytes = await blob.bytes();
    const identity = layerIdentity(index, layer, bytes);
    try {
      // Decode once so GNU tar and Bun inspect the same uncompressed archive.
      if (bytes[0] === 0x1f && bytes[1] === 0x8b) {
        const decoded = gunzipSync(bytes, { info: true }) as unknown as {
          buffer: Uint8Array;
          engine: { bytesWritten: number };
        };
        // Gunzip can stop at NUL padding and silently discard the remaining input.
        check(decoded.engine.bytesWritten === bytes.length, "unconsumed gzip input");
        bytes = decoded.buffer;
      } else if (bytes[0] === 0x28 && bytes[1] === 0xb5 && bytes[2] === 0x2f && bytes[3] === 0xfd) {
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
    const files = await new Bun.Archive(bytes).files();
    for (const [name, file] of files) {
      check(
        basename(name) !== ".env" && !basename(name).startsWith(".env."),
        "environment file found; content withheld",
      );
      const head = await file.slice(0, 1024 * 1024).bytes();
      const text = !head.includes(0); // compiled binaries contain protocol literals
      let tail = "";
      for await (const chunk of file.stream()) {
        const data = tail + Buffer.from(chunk).toString("latin1");
        checkText(data, text); // token signatures also checked in binaries
        tail = data.slice(-1024);
      }
    }
  }
  console.log("Image metadata and all saved layers: credential/env/DB URL checks passed");
}

export function buildSummary(log: string, info?: any) {
  const rows = ["\n| Build step | Seconds |", "| --- | ---: |"];
  const descriptions = new Map<string, string>();
  for (const line of log.split("\n")) {
    const description = line.match(/^(#\d+) (\[.*?\] .*)/);
    if (description) descriptions.set(description[1], description[2].replaceAll("|", "\\|"));
    const done = line.match(/^(#\d+) (?:DONE ([0-9.]+)s|(CACHED))$/);
    if (done && descriptions.has(done[1]))
      rows.push(`| ${descriptions.get(done[1])} | ${done[2] ?? "cached"} |`);
  }
  if (info) rows.push(`\nUncompressed image size: ${info.Size} bytes (${info.Architecture}).`);
  return rows.join("\n");
}

export function archDigest(config: any, manifest: any, arch: string) {
  check(
    config.architecture === arch && config.os === "linux" && config.config.User === "1000:1000",
    "published architecture/user mismatch",
  );
  check(/^sha256:[0-9a-f]{64}$/.test(manifest.digest), "invalid architecture digest");
  return manifest.digest;
}
export function verifyManifest(index: any, sources: string[]) {
  check(
    index.manifests.length === 2 && /^sha256:[0-9a-f]{64}$/.test(index.digest),
    "invalid multi-arch manifest",
  );
  check(
    Bun.deepEquals(
      index.manifests.map((m: any) => `${m.platform.os}/${m.platform.architecture}`).sort(),
      ["linux/amd64", "linux/arm64"],
    ),
    "wrong manifest platforms",
  );
  check(
    Bun.deepEquals(
      index.manifests.map((m: any) => m.digest).sort(),
      sources.map((s) => s.split("@")[1]).sort(),
    ),
    "manifest must use verified source digests",
  );
}

if (import.meta.main) {
  const [command, ...args] = process.argv.slice(2);
  try {
    switch (command) {
      case "inputs": {
        const rust: any = Bun.TOML.parse(await Bun.file("rust-toolchain.toml").text());
        check(
          rust.toolchain.channel === "1.98.1" &&
            (await Bun.file(".bun-version").text()).trim() === "1.4.2",
          "tool versions differ from recipe",
        );
        check(
          (await json("apps/web/package.json")).devDependencies["@playwright/test"] === "1.63.0",
          "Playwright pin differs from recipe",
        );
        verifyWorkflow(
          Bun.YAML.parse(await Bun.file(".github/workflows/ci-base-image.yml").text()),
        );
        break;
      }
      case "scan":
        await scanImage(args[0], args[1], args[2]);
        break;
      case "summary":
        console.log(
          buildSummary(
            await Bun.file(args[0]).text(),
            (await Bun.file(args[1]).exists()) ? await json(args[1]) : undefined,
          ),
        );
        break;
      case "arch-digest":
        console.log(archDigest(await json(args[0]), await json(args[1]), args[2]));
        break;
      case "layer-size": {
        const manifest = await json(args[0]);
        console.log(
          `Compressed layer size (${args[1]}): ${manifest.layers.reduce((size: number, layer: any) => size + layer.size, 0)} bytes`,
        );
        break;
      }
      case "manifest": {
        const index = await json(args[0]);
        verifyManifest(index, args.slice(2));
        console.log(`Final multi-arch image: ${args[1]}@${index.digest}`);
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
