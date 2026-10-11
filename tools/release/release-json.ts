// JSON checks of scripts/release-publish.sh and scripts/release-existing.sh.
// Each prints the value the script needs, or exits 1 with the reason on stderr.
//
//   version FILE TAG              release.json version, when "v"+version is TAG
//   field FILE KEY                a string field of release.json (indexDigest, image)
//   state JSON                    the state of a release-api.ts release-state answer
//   assets-complete JSON NAME...  exit 1 unless the release lists every NAME
//   record-digest FILE VERSION SHA  indexDigest of a release.json for VERSION at SHA
//   image-labels FILE VERSION SHA   `imagetools inspect --format '{{json .Image}}'`
//                                   is linux/amd64 + linux/arm64 labelled VERSION and SHA
import { readFileSync } from "node:fs";
import { EXIT_USAGE, Fail, parseJson, repr, str } from "./fail.ts";

type Json = Record<string, unknown>;

function object(value: unknown, what: string): Json {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new Fail(`${what} is not a JSON object`);
  return value as Json;
}

function stringField(record: Json, key: string, what: string): string {
  const value = record[key];
  if (typeof value !== "string") throw new Fail(`${what} has no string ${key} (${repr(value)})`);
  return value;
}

export function version(record: Json, tag: string): string {
  const value = stringField(record, "version", "release.json");
  if (`v${value}` !== tag)
    throw new Fail(`release.json version ${repr(value)} does not match tag ${repr(tag)}`);
  return value;
}

export function state(answer: Json): string {
  return stringField(answer, "state", "release state");
}

export function missingAssets(answer: Json, required: string[]): string[] {
  const listed = answer.assets;
  if (!Array.isArray(listed) || !listed.every((name) => typeof name === "string")) {
    throw new Fail("release state has no asset name list");
  }
  const present = new Set(listed);
  return [...new Set(required)].filter((name) => !present.has(name)).sort();
}

export function recordDigest(record: Json, expectedVersion: string, sha: string): string {
  if (record.version !== expectedVersion || record.sourceSha !== sha) {
    throw new Fail(
      `existing release records ${str(record.version)} at ${str(record.sourceSha)}, not ${expectedVersion} at ${sha}`,
    );
  }
  return stringField(record, "indexDigest", "release.json");
}

export function checkImageLabels(images: Json, expectedVersion: string, sha: string): void {
  const platforms = Object.keys(images).sort();
  if (platforms.length !== 2 || platforms[0] !== "linux/amd64" || platforms[1] !== "linux/arm64") {
    throw new Fail(`image platforms ${repr(platforms)}`);
  }
  for (const platform of platforms) {
    const image = object(images[platform], platform);
    const config = image.config === undefined ? {} : object(image.config, `${platform} config`);
    const labels =
      config.Labels === undefined || config.Labels === null
        ? {}
        : object(config.Labels, `${platform} labels`);
    if (
      labels["org.opencontainers.image.version"] !== expectedVersion ||
      labels["org.opencontainers.image.revision"] !== sha
    ) {
      throw new Fail(
        `${platform}: labels version ${str(labels["org.opencontainers.image.version"])} revision ${str(labels["org.opencontainers.image.revision"])}`,
      );
    }
  }
}

function readJson(path: string): Json {
  // Bytes, not a "utf8" string read: invalid UTF-8 must be refused, not replaced
  // by U+FFFD; a BOM is kept so JSON.parse refuses it as the original did.
  let text: string;
  try {
    text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(readFileSync(path));
  } catch (error) {
    if (error instanceof TypeError) throw new Fail(`${path} is not valid UTF-8`);
    throw error;
  }
  return object(parseJson(text, path), path);
}

export function run(argv: string[]): string | undefined {
  const [command = "", ...rest] = argv;
  const arity = (n: number) => {
    if (rest.length !== n)
      throw new Fail(`${command}: expected ${String(n)} arguments`, EXIT_USAGE);
  };
  const arg = (i: number) => rest[i] ?? "";
  switch (command) {
    case "version":
      arity(2);
      return version(readJson(arg(0)), arg(1));
    case "field":
      arity(2);
      return stringField(readJson(arg(0)), arg(1), arg(0));
    case "state":
      arity(1);
      return state(object(parseJson(arg(0), "release state"), "release state"));
    case "assets-complete": {
      if (rest.length < 1)
        throw new Fail(`${command}: expected a state and asset names`, EXIT_USAGE);
      const missing = missingAssets(
        object(parseJson(arg(0), "release state"), "release state"),
        rest.slice(1),
      );
      if (missing.length > 0) throw new Fail(`missing ${repr(missing)}`);
      return undefined;
    }
    case "record-digest":
      arity(3);
      return recordDigest(readJson(arg(0)), arg(1), arg(2));
    case "image-labels":
      arity(3);
      checkImageLabels(readJson(arg(0)), arg(1), arg(2));
      return undefined;
    default:
      throw new Fail(`unknown command ${repr(command)}`, EXIT_USAGE);
  }
}

if (import.meta.main) {
  try {
    const value = run(process.argv.slice(2));
    if (value !== undefined) process.stdout.write(value + "\n");
  } catch (error) {
    const fail =
      error instanceof Fail
        ? error
        : new Fail(error instanceof Error ? error.message : String(error));
    process.stderr.write(`release-json: ${fail.message}\n`);
    process.exitCode = fail.code;
  }
}
