// Registry and GitHub release calls for the release workflow (docs/RELEASING.md).
// Decisions use HTTP status codes and JSON fields, never error-message wording.
//
//   registry-digest --image ghcr.io/aisflow/fvoci --tag 0.y.z
//       manifest digest of a tag, or an empty line when it does not exist;
//       exit 4 when the registry refuses to answer (401/403).
//   push-index --image IMAGE --amd64 sha256:... --arm64 sha256:...
//       pushes a two-platform index BY DIGEST (no tag) and prints its digest.
//   describe --image IMAGE --digest sha256:...
//       index_digest/amd64_digest/arm64_digest lines for a two-platform linux
//       index (fails on anything else).
//   tag --image IMAGE --digest sha256:... --tag 0.y.z [--floating]
//       points a tag at an existing index. Without --floating the tag is
//       immutable: an existing tag with another digest fails. With --floating
//       the tag moves. Either way the pushed bytes are the digest's own bytes.
//   release-state --tag v0.y.z
//       {"state": "none"|"published"|"draft", "assets": [...]}.
//
// Registry credentials: REGISTRY_USER and REGISTRY_PASSWORD (anonymous when
// unset). GitHub: GH_TOKEN and GITHUB_REPOSITORY (GITHUB_API_URL optional).
import { parseArgs } from "node:util";
import { EXIT_USAGE, Fail, repr } from "./fail.ts";
import { releaseState, REPOSITORY, formatReleaseState } from "./github.ts";
import { fetchClient, type HttpClient } from "./http.ts";
import { Registry, type Credentials } from "./registry-client.ts";
import {
  ACCEPT_IMAGE,
  ACCEPT_INDEX,
  buildIndex,
  DIGEST,
  DOCKER_LIST,
  DOCKER_MANIFEST,
  indexPlatforms,
  OCI_INDEX,
  OCI_MANIFEST,
  sha256Digest,
  TAG,
  type PlatformManifest,
} from "./registry.ts";

type Env = Record<string, string | undefined>;
type Args = Record<string, string | boolean | undefined>;

interface Context {
  http: HttpClient;
  env: Env;
  out: (line: string) => void;
  err: (line: string) => void;
}

interface Command {
  strings: string[];
  booleans?: string[];
  run: (args: Args, ctx: Context) => Promise<void>;
}

const credentials = (env: Env): Credentials => ({
  user: env.REGISTRY_USER,
  password: env.REGISTRY_PASSWORD,
});
const text = (args: Args, key: string): string => String(args[key]);

const COMMANDS: Record<string, Command> = {
  "registry-digest": {
    strings: ["image", "tag"],
    async run(args, { http, env, out }) {
      const tag = text(args, "tag");
      if (!TAG.test(tag)) throw new Fail(`invalid tag ${repr(tag)}`, EXIT_USAGE);
      const registry = await Registry.open(http, text(args, "image"), "pull", credentials(env));
      out((await registry.manifest(tag, ACCEPT_INDEX))?.digest ?? "");
    },
  },

  "push-index": {
    strings: ["image", "amd64", "arm64"],
    async run(args, { http, env, out }) {
      const image = text(args, "image");
      const wanted = { amd64: text(args, "amd64"), arm64: text(args, "arm64") };
      for (const [arch, digest] of Object.entries(wanted)) {
        if (!DIGEST.test(digest)) throw new Fail(`--${arch} must be sha256:<64 hex>`, EXIT_USAGE);
      }
      const registry = await Registry.open(http, image, "pull,push", credentials(env));
      const platform = async (arch: string, digest: string): Promise<PlatformManifest> => {
        const found = await registry.manifest(digest, ACCEPT_IMAGE);
        if (found === null) throw new Fail(`${image}@${digest} (${arch}) does not exist`);
        if (found.mediaType !== OCI_MANIFEST && found.mediaType !== DOCKER_MANIFEST) {
          throw new Fail(
            `${image}@${digest} (${arch}) is ${repr(found.mediaType)}, not a single-platform image manifest`,
          );
        }
        return { mediaType: found.mediaType, digest, size: found.body.byteLength };
      };
      const amd64 = await platform("amd64", wanted.amd64);
      const arm64 = await platform("arm64", wanted.arm64);
      const index = buildIndex(amd64, arm64);
      out(await registry.put(sha256Digest(index.body), index.mediaType, index.body));
    },
  },

  describe: {
    strings: ["image", "digest"],
    async run(args, { http, env, out }) {
      const image = text(args, "image");
      const digest = text(args, "digest");
      if (!DIGEST.test(digest)) throw new Fail("--digest must be sha256:<64 hex>", EXIT_USAGE);
      const registry = await Registry.open(http, image, "pull", credentials(env));
      const found = await registry.manifest(digest, ACCEPT_INDEX);
      if (found === null) throw new Fail(`${image}@${digest} does not exist`);
      if (found.mediaType !== OCI_INDEX && found.mediaType !== DOCKER_LIST) {
        throw new Fail(`${image}@${digest} is ${repr(found.mediaType)}, not an index`);
      }
      const index = JSON.parse(new TextDecoder().decode(found.body)) as Record<string, unknown>;
      const platforms = indexPlatforms(index, `${image}@${digest}`);
      out(`index_digest=${digest}`);
      out(`amd64_digest=${platforms.amd64}`);
      out(`arm64_digest=${platforms.arm64}`);
    },
  },

  tag: {
    strings: ["image", "digest", "tag"],
    booleans: ["floating"],
    async run(args, { http, env, err }) {
      const image = text(args, "image");
      const digest = text(args, "digest");
      const tag = text(args, "tag");
      if (!DIGEST.test(digest) || !TAG.test(tag)) {
        throw new Fail("--digest must be sha256:<64 hex> and --tag a registry tag", EXIT_USAGE);
      }
      const registry = await Registry.open(http, image, "pull,push", credentials(env));
      const found = await registry.manifest(digest, ACCEPT_INDEX);
      if (found === null) throw new Fail(`${image}@${digest} does not exist`);
      const current = await registry.manifest(tag, ACCEPT_INDEX);
      if (current?.digest === digest) {
        err(`${image}:${tag} already ${digest}`);
        return;
      }
      if (current && !args.floating) {
        throw new Fail(
          `${image}:${tag} is ${current.digest}, not ${digest}; immutable tags are never moved`,
        );
      }
      await registry.put(tag, found.mediaType, found.body);
      const after = await registry.manifest(tag, ACCEPT_INDEX);
      if (after?.digest !== digest) {
        throw new Fail(`${image}:${tag} reads back as ${after?.digest ?? "None"}, not ${digest}`);
      }
      err(`${image}:${tag} -> ${digest}` + (current ? ` (was ${current.digest})` : ""));
    },
  },

  "release-state": {
    strings: ["tag"],
    async run(args, { http, env, out }) {
      const repository = env.GITHUB_REPOSITORY ?? "";
      const token = env.GH_TOKEN ?? "";
      if (!REPOSITORY.test(repository) || !token)
        throw new Fail("GITHUB_REPOSITORY and GH_TOKEN are required", EXIT_USAGE);
      const api = env.GITHUB_API_URL ?? "https://api.github.com";
      out(formatReleaseState(await releaseState(http, api, repository, token, text(args, "tag"))));
    },
  },
};

function parse(argv: string[]): { command: Command; args: Args } {
  const [name = "", ...rest] = argv;
  const command = Object.hasOwn(COMMANDS, name) ? COMMANDS[name] : undefined;
  if (command === undefined) {
    throw new Fail(`usage: release-api.ts {${Object.keys(COMMANDS).join(",")}} ...`, EXIT_USAGE);
  }
  const options: Record<string, { type: "string" | "boolean" }> = {};
  for (const key of command.strings) options[key] = { type: "string" };
  for (const key of command.booleans ?? []) options[key] = { type: "boolean" };
  let values: Args;
  try {
    values = parseArgs({ args: rest, options, strict: true, allowPositionals: false }).values;
  } catch (error) {
    throw new Fail(`${name}: ${(error as Error).message}`, EXIT_USAGE);
  }
  const missing = command.strings.filter((key) => values[key] === undefined);
  if (missing.length > 0) {
    throw new Fail(
      `${name}: the following arguments are required: ${missing.map((k) => `--${k}`).join(", ")}`,
      EXIT_USAGE,
    );
  }
  return { command, args: values };
}

/** Runs one subcommand; returns the exit code. Refusals go to `err` with the release-api prefix. */
export async function main(
  argv: string[],
  env: Env,
  http: HttpClient,
  out: (line: string) => void,
  err: (line: string) => void,
): Promise<number> {
  try {
    const { command, args } = parse(argv);
    await command.run(args, { http, env, out, err });
    return 0;
  } catch (error) {
    const fail =
      error instanceof Fail
        ? error
        : new Fail(error instanceof Error ? error.message : String(error));
    err(`release-api: ${fail.message}`);
    return fail.code;
  }
}

if (import.meta.main) {
  process.exitCode = await main(
    process.argv.slice(2),
    process.env,
    fetchClient,
    (line) => process.stdout.write(line + "\n"),
    (line) => process.stderr.write(line + "\n"),
  );
}
