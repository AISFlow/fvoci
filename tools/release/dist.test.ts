// Dry runs of scripts/release-dist.sh, tools/release/provenance.ts and
// scripts/release-preflight.sh. Each case copies the scripts and
// tools/release into a scratch root with a user compose, its env example and
// start guide (the testdata copies of infra/rust/compose.user.*, and the real
// files when the tree has them) and runs them as the release workflow does.
// No registry, no network; the preflight cases need the docker CLI with the
// compose plugin (no daemon).
import { afterEach, describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const ROOT = resolve(import.meta.dir, "../..");
const FIXTURE = join(ROOT, "scripts/testdata/release/compose.user.yml");
const REAL = join(ROOT, "infra/rust/compose.user.yml");
const SIDECARS = [".env.example", ".INSTALL.md"];
const IMAGE = "ghcr.io/aisflow/fvoci";
const VERSION = "0.1.0";
const INDEX = "sha256:" + "1".repeat(64);
const AMD64 = "sha256:" + "2".repeat(64);
const ARM64 = "sha256:" + "3".repeat(64);
const SHA = "a".repeat(40);
const TOOLING_SHA = "b".repeat(40);
const PINNED = `${IMAGE}:${VERSION}@${INDEX}`;
const DOCKERFILE = `FROM rust:1 AS rust-sources
FROM rust-sources AS rust-build
ARG FVOCI_BUILD_SHA=
ENV FVOCI_BUILD_SHA=\${FVOCI_BUILD_SHA}
FROM debian:bookworm-slim AS runtime
`;
const SUMMED = ["compose.yml", "env.example", "INSTALL.md", "release.json", "RELEASE-NOTES.md"];

const composeSources = () => [FIXTURE, ...(existsSync(REAL) ? [REAL] : [])];
const sidecar = (source: string, suffix: string) => source.replace(/\.yml$/, suffix);
const read = (path: string) => readFileSync(path, "utf8");
const sha256 = (path: string) => createHash("sha256").update(readFileSync(path)).digest("hex");

type Run = { code: number; stdout: string; stderr: string };

function run(cmd: string[]): Run {
  const proc = Bun.spawnSync(cmd, { stdout: "pipe", stderr: "pipe" });
  return { code: proc.exitCode, stdout: proc.stdout.toString(), stderr: proc.stderr.toString() };
}

const scratches: string[] = [];
afterEach(() => {
  for (const dir of scratches.splice(0)) rmSync(dir, { recursive: true, force: true });
});

class Scratch {
  readonly root = mkdtempSync(join(tmpdir(), "fvoci-release-dist-"));

  constructor(compose: string, source = FIXTURE) {
    scratches.push(this.root);
    mkdirSync(join(this.root, "scripts"));
    mkdirSync(join(this.root, "infra/rust"), { recursive: true });
    mkdirSync(join(this.root, "tools/release"), { recursive: true });
    for (const name of ["release-dist.sh", "release-preflight.sh", "release-notes-template.md"]) {
      copyFileSync(join(ROOT, "scripts", name), join(this.root, "scripts", name));
    }
    for (const name of readdirSync(join(ROOT, "tools/release"))) {
      if (name.endsWith(".ts") && !name.endsWith(".test.ts")) {
        copyFileSync(join(ROOT, "tools/release", name), join(this.root, "tools/release", name));
      }
    }
    this.write("infra/rust/compose.user.yml", compose);
    for (const suffix of SIDECARS) {
      copyFileSync(sidecar(source, suffix), join(this.root, `infra/rust/compose.user${suffix}`));
    }
    this.write("infra/rust/Dockerfile", DOCKERFILE);
  }

  write(rel: string, text: string): void {
    writeFileSync(join(this.root, rel), text);
  }

  read(rel: string): string {
    return read(join(this.root, rel));
  }

  dist(): Run {
    return run([
      "bash",
      join(this.root, "scripts/release-dist.sh"),
      ...["--version", VERSION, "--sha", SHA, "--repository", "AISFlow/fvoci", "--image", IMAGE],
      ...["--index-digest", INDEX, "--amd64-digest", AMD64, "--arm64-digest", ARM64],
      ...["--run-url", "https://github.com/AISFlow/fvoci/actions/runs/1"],
      ...["--out", join(this.root, "dist")],
    ]);
  }

  stamp(sha = TOOLING_SHA, ref = "refs/heads/main"): Run {
    return run([
      process.execPath,
      join(this.root, "tools/release/provenance.ts"),
      ...["--dist", join(this.root, "dist"), "--tooling-sha", sha, "--tooling-ref", ref],
    ]);
  }

  preflight(): Run {
    return run(["bash", join(this.root, "scripts/release-preflight.sh"), "--version", VERSION]);
  }

  readyNotes(): void {
    const notes = this.read("scripts/release-notes-template.md")
      .split("\n")
      .map((line) =>
        line.includes("notes-for:")
          ? `<!-- notes-for: ${VERSION} -->`
          : line.includes("TODO(release)")
            ? "- written for this release"
            : line,
      );
    if (notes.at(-1) === "") notes.pop();
    this.write("scripts/release-notes-template.md", notes.join("\n") + "\n");
  }
}

function expectRejected(scratch: Scratch, needle: string): void {
  const proc = scratch.dist();
  expect(proc.code, proc.stdout).not.toBe(0);
  expect(proc.stderr).toContain(needle);
}

describe("release-dist", () => {
  test.each(composeSources())("renders the user compose anchor (%s)", (source) => {
    const s = new Scratch(read(source), source);
    const proc = s.dist();
    expect(proc.code, proc.stderr).toBe(0);
    const rendered = s.read("dist/compose.yml");
    expect(rendered).toContain(`x-fvoci-image: &fvoci-image ${PINNED}\n`);
    expect(rendered.split(IMAGE).length - 1).toBe(1);
    expect(rendered).not.toContain("FVOCI_IMAGE");
    expect(rendered.split("image: *fvoci-image").length - 1).toBe(1);
    expect(s.read("dist/env.example")).toBe(read(sidecar(source, ".env.example")));
    expect(s.read("dist/INSTALL.md")).toContain("cp env.example .env");
    expect(s.read("dist/env.example")).toContain("POSTGRES_PASSWORD=\n");
    const record = JSON.parse(s.read("dist/release.json")) as Record<string, unknown>;
    expect(record.image).toBe(PINNED);
    expect(record.platforms).toEqual({ "linux/amd64": AMD64, "linux/arm64": ARM64 });
    expect(record.composeSource).toBe("infra/rust/compose.user.yml");
    expect(record.files).toEqual(["compose.yml", "env.example", "INSTALL.md"]);
    expect(record.tags).toEqual({ immutable: "0.1.0", floating: "0.1" });
    const order = record.publishOrder as string[];
    expect(order.slice(0, 3)).toEqual([
      "index-by-digest",
      "smoke-linux/amd64",
      "smoke-linux/arm64",
    ]);
    expect(order.at(-1)).toBe("github-release");
    const sums = new Map(
      s
        .read("dist/SHA256SUMS")
        .trimEnd()
        .split("\n")
        .map((line) => line.split("  ").reverse() as [string, string]),
    );
    expect(sums.size).toBe(5);
    for (const name of SUMMED) expect(sums.get(name)).toBe(sha256(join(s.root, "dist", name)));
  });

  const base = read(FIXTURE);
  const anchor = "x-fvoci-image: &fvoci-image ${FVOCI_IMAGE:-ghcr.io/aisflow/fvoci:0.1.0}";
  const rejected: Array<[string, string, string]> = [
    [
      "no default",
      base.replace(anchor, "x-fvoci-image: &fvoci-image ${FVOCI_IMAGE}"),
      "expected exactly one",
    ],
    [
      "required form",
      base.replace(anchor, "x-fvoci-image: &fvoci-image ${FVOCI_IMAGE:?set it}"),
      "expected exactly one",
    ],
    [
      "no anchor",
      base.replace(anchor, "x-other: &fvoci-image ${FVOCI_IMAGE:-ghcr.io/aisflow/fvoci:0.1.0}"),
      "expected exactly one",
    ],
    ["second anchor", base + "\n" + anchor + "\n", "expected exactly one"],
    [
      "variable elsewhere",
      base.replace("    mem_limit: 4g", '    mem_limit: 4g\n    labels: ["${FVOCI_IMAGE}"]'),
      "only in the x-fvoci-image anchor",
    ],
    [
      "hard-coded image",
      base.replace(
        "  fvoci:\n    image: *fvoci-image",
        "  fvoci:\n    image: ghcr.io/aisflow/fvoci:latest",
      ),
      "only through the x-fvoci-image anchor",
    ],
    ["no alias", base.replace("image: *fvoci-image", "image: busybox"), "no service uses"],
    [
      "optional variable",
      base.replace(
        "${FVOCI_PUBLIC_ORIGIN:?set FVOCI_PUBLIC_ORIGIN in .env}",
        "${FVOCI_PUBLIC_ORIGIN:-http://localhost:8080}",
      ),
      "every interpolation must be ${VAR:?message}",
    ],
    [
      "variable not in env.example",
      base.replace('FVOCI_COLLAB_MAX_ROOMS: "64"', 'FVOCI_COLLAB_MAX_ROOMS: "${ROOMS:?set ROOMS}"'),
      "missing ['ROOMS']",
    ],
    [
      "env file",
      base.replace("    mem_limit: 4g", "    mem_limit: 4g\n    env_file: .env"),
      "env_file",
    ],
  ];

  test("the fixture carries the anchor the rejection cases edit", () => {
    expect(base).toContain(anchor);
    for (const [, compose] of rejected) expect(compose).not.toBe(base);
  });

  test.each(rejected)("rejects other image forms: %s", (_name, compose, needle) => {
    expectRejected(new Scratch(compose), needle);
  });

  test("rejects an unused env.example variable", () => {
    const s = new Scratch(base);
    s.write(
      "infra/rust/compose.user.env.example",
      s.read("infra/rust/compose.user.env.example") + "SMTP_HOST=\n",
    );
    expectRejected(s, "unused ['SMTP_HOST']");
  });
});

describe("release provenance", () => {
  function rendered(): Scratch {
    const s = new Scratch(read(FIXTURE));
    const proc = s.dist();
    expect(proc.code, proc.stderr).toBe(0);
    return s;
  }

  test("records both commits and keeps the product bound to the tag", () => {
    const s = rendered();
    const before = ["compose.yml", "env.example", "INSTALL.md"].map((n) => s.read(`dist/${n}`));
    const recordBefore = JSON.parse(s.read("dist/release.json")) as Record<string, unknown>;
    const proc = s.stamp();
    expect(proc.code, proc.stderr).toBe(0);
    const record = JSON.parse(s.read("dist/release.json")) as Record<string, unknown>;
    expect(record).toEqual({
      ...recordBefore,
      toolingSha: TOOLING_SHA,
      toolingRef: "refs/heads/main",
    });
    expect(record.sourceSha).toBe(SHA);
    expect(["compose.yml", "env.example", "INSTALL.md"].map((n) => s.read(`dist/${n}`))).toEqual(
      before,
    );
    const notes = s.read("dist/RELEASE-NOTES.md");
    expect(notes).toContain(`\`${SHA}\` (tag v${VERSION})`);
    expect(notes).toContain(`Release smoke tooling: \`${TOOLING_SHA}\` (refs/heads/main)`);
    const check = Bun.spawnSync(["sha256sum", "--strict", "-c", "SHA256SUMS"], {
      cwd: join(s.root, "dist"),
      stdout: "pipe",
      stderr: "pipe",
    });
    expect(check.exitCode, check.stdout.toString() + check.stderr.toString()).toBe(0);
    expect(s.read("dist/SHA256SUMS").trimEnd().split("\n")).toHaveLength(5);
  });

  test("refuses a second stamp, bad input or tampered files", () => {
    const s = rendered();
    const bad: Array<[string, string, string]> = [
      ["B".repeat(40), "refs/heads/main", "full commit SHA"],
      [TOOLING_SHA, "main", "refs/heads/<name>"],
    ];
    for (const [sha, ref, needle] of bad) {
      const proc = s.stamp(sha, ref);
      expect(proc.code).toBe(1);
      expect(proc.stderr).toContain(needle);
    }
    expect(s.stamp().code).toBe(0);
    const again = s.stamp();
    expect(again.code).toBe(1);
    expect(again.stderr).toContain("already stamped");
    const t = rendered();
    t.write("dist/compose.yml", t.read("dist/compose.yml") + "# edited\n");
    const tampered = t.stamp();
    expect(tampered.code).toBe(1);
    expect(tampered.stderr).toContain("does not match compose.yml");
  });

  // Stricter than argparse: no option abbreviations, no repeated option.
  test.each([
    [["--dist", "D"], "missing --tooling-sha, --tooling-ref"],
    [["--dist", "D", "--tooling-s", "x", "--tooling-ref", "refs/heads/main"], "tooling-s"],
    [["--dist=D", "--dist", "D", "--tooling-sha=x", "--tooling-ref=y"], "--dist given twice"],
    [["--dist", "D", "extra", "--tooling-sha=x", "--tooling-ref=y"], "extra"],
  ])("refuses usage error %j with exit 2 and leaves the files", (argv, needle) => {
    const s = rendered();
    const before = s.read("dist/SHA256SUMS");
    const args = argv.map((arg) => arg.replace(/^D$|(?<==)D$/, join(s.root, "dist")));
    const proc = run([process.execPath, join(s.root, "tools/release/provenance.ts"), ...args]);
    expect(proc.code).toBe(2);
    expect(proc.stderr).toContain(needle);
    expect(s.read("dist/SHA256SUMS")).toBe(before);
  });

  test("refuses a SHA256SUMS that lists a file twice", () => {
    const s = rendered();
    const sums = s.read("dist/SHA256SUMS");
    s.write("dist/SHA256SUMS", sums + (sums.split("\n")[0] ?? "") + "\n");
    const proc = s.stamp();
    expect(proc.code).toBe(1);
    expect(proc.stderr).toContain("SHA256SUMS lists 'compose.yml' twice");
  });
});

describe("release preflight", () => {
  function ready(source = FIXTURE): Scratch {
    const s = new Scratch(read(source), source);
    s.readyNotes();
    return s;
  }

  test.each(composeSources())("passes when ready (%s)", (source) => {
    const proc = ready(source).preflight();
    expect(proc.code, proc.stderr).toBe(0);
    expect(proc.stdout).toContain("app: ['fvoci']");
    expect(proc.stdout).toContain("unfilled env.example refused");
  });

  test("refuses unwritten notes", () => {
    const s = ready();
    s.write(
      "scripts/release-notes-template.md",
      s.read("scripts/release-notes-template.md") + "<!-- TODO(release): known limitations -->\n",
    );
    const proc = s.preflight();
    expect(proc.code).toBe(1);
    expect(proc.stderr).toContain("TODO(release) markers");
  });

  test("refuses notes for another version", () => {
    const s = ready();
    s.write(
      "scripts/release-notes-template.md",
      s
        .read("scripts/release-notes-template.md")
        .replace(`notes-for: ${VERSION}`, "notes-for: 0.0.9"),
    );
    const proc = s.preflight();
    expect(proc.code).toBe(1);
    expect(proc.stderr).toContain(`notes-for: ${VERSION}`);
  });

  test.each([
    DOCKERFILE.replace("ARG FVOCI_BUILD_SHA=\n", ""),
    DOCKERFILE.replace("ARG FVOCI_BUILD_SHA=\n", "") + "ARG FVOCI_BUILD_SHA=\n",
  ])("refuses a Dockerfile without the build SHA in rust-build (%#)", (dockerfile) => {
    const s = ready();
    s.write("infra/rust/Dockerfile", dockerfile);
    const proc = s.preflight();
    expect(proc.code).toBe(1);
    expect(proc.stderr).toContain("ARG FVOCI_BUILD_SHA");
  });

  test("refuses a compose without one published app", () => {
    const s = ready();
    const compose = s.read("infra/rust/compose.user.yml");
    const published = ':?set FVOCI_PUBLISH_PORT in .env}:8080"';
    expect(compose).toContain(published);
    s.write(
      "infra/rust/compose.user.yml",
      compose.replace(published, published.replace(":8080", ":9090")),
    );
    const proc = s.preflight();
    expect(proc.code).toBe(1);
    expect(proc.stderr).toContain("expected one service publishing container port 8080, found []");
  });

  const appKey = "      ENCRYPTION_KEYS: ${ENCRYPTION_KEYS:?set ENCRYPTION_KEYS in .env}\n";
  test.each([
    [
      "app keyring in postgres",
      (c: string) => c.replace("      POSTGRES_DB: fvoci\n", "      POSTGRES_DB: fvoci\n" + appKey),
      "postgres gets .env values it does not need: ['ENCRYPTION_KEYS']",
    ],
    [
      "owner password in meilisearch",
      (c: string) =>
        c.replace(
          '      MEILI_NO_ANALYTICS: "true"\n',
          '      MEILI_NO_ANALYTICS: "true"\n      DB: "x:${POSTGRES_PASSWORD:?set POSTGRES_PASSWORD in .env}"\n',
        ),
      "meilisearch gets .env values it does not need: ['POSTGRES_PASSWORD']",
    ],
    [
      "master key on a command line",
      (c: string) =>
        c.replace(
          "    environment:\n      MEILI_ENV: production\n",
          '    command: ["meilisearch", "--master-key", "${MEILI_MASTER_KEY:?set MEILI_MASTER_KEY in .env}"]\n' +
            "    environment:\n      MEILI_ENV: production\n",
        ),
      "meilisearch has .env values outside its environment: ['MEILI_MASTER_KEY']",
    ],
  ] as const)("refuses values a service does not need: %s", (_name, edit, needle) => {
    const s = ready();
    const compose = s.read("infra/rust/compose.user.yml");
    expect(edit(compose)).not.toBe(compose);
    s.write("infra/rust/compose.user.yml", edit(compose));
    const proc = s.preflight();
    expect(proc.code, proc.stdout).toBe(1);
    expect(proc.stderr).toContain(needle);
  });
});
