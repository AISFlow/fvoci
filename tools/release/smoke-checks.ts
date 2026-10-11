// Checks and small helpers for scripts/release-smoke.sh. Each subcommand
// exits 1 with a "release-smoke: ..." line when its check fails.
//
//   json-get [--default VALUE] JSON KEY...   value at that path (digit keys index lists)
//   index-platforms INDEX_JSON RECORD_PATH    the index's per-platform digests equal the record's
//   labels LABELS_JSON VERSION SHA            OCI version/revision labels
//   fill-env EXAMPLE OUT                      fresh value for every empty entry
//   set-env ENV_FILE KEY VALUE                replace the one KEY= line
//   app-service < CONFIG_JSON                 the one service publishing 8080
//   product-services IMAGE APP < CONFIG_JSON  APP runs the product image, no env_file
//   doctor-ok < REPORT_JSON                   the doctor report says ok
//   uuid                                      a random UUID
//   url-quote TEXT                            TEXT percent-encoded for a query value
//   search-has ID < RESULTS_JSON              exit 0 when an item names ID, else 1
//   body-equal EXPECTED_JSON < BODY_JSON      the contentJson of both is equal
import { randomBytes, randomUUID } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { appServices, productServices } from "./dist-check.ts";
import {
  ReleaseError,
  isRecord,
  jsonEqual,
  readText,
  repr,
  splitLines,
  truthy,
  universalNewlines,
} from "./py.ts";

function display(value: unknown): string {
  return typeof value === "string" ? value : repr(value);
}

export function jsonGet(value: unknown, keys: string[]): unknown {
  let node = value;
  for (const key of keys) {
    if (/^\d+$/.test(key)) {
      const index = Number(key);
      if (Array.isArray(node)) node = node[index];
      else throw new ReleaseError(`cannot index ${repr(node)} with ${key}`);
      if (node === undefined) throw new ReleaseError(`index ${key} out of range`);
    } else {
      if (!isRecord(node) || !Object.hasOwn(node, key)) {
        throw new ReleaseError(`no key ${repr(key)}`);
      }
      node = node[key];
    }
  }
  return node;
}

export function indexPlatforms(index: unknown): Record<string, unknown> {
  const manifests = isRecord(index) && Array.isArray(index.manifests) ? index.manifests : [];
  const found: Record<string, unknown> = {};
  for (const manifest of manifests) {
    if (!isRecord(manifest)) throw new ReleaseError("index manifest is not an object");
    const platform = manifest.platform ?? {};
    if (isRecord(platform) && platform.os === "unknown") continue;
    if (!isRecord(platform) || !("os" in platform) || !("architecture" in platform)) {
      throw new ReleaseError(`index manifest without platform: ${JSON.stringify(manifest)}`);
    }
    if (!("digest" in manifest)) throw new ReleaseError("index manifest without digest");
    found[`${display(platform.os)}/${display(platform.architecture)}`] = manifest.digest;
  }
  return found;
}

const ENV_LINE = /^([A-Z][A-Z0-9_]*)=$/;

// Every empty entry gets a fresh value in the format its comment shows
// (openssl rand -hex 32; a *_KEYS keyring under its *_ACTIVE_KEY_ID).
// Filled entries are kept.
export function fillEnv(example: string, token: () => string): string {
  const values = new Map<string, string>();
  for (const m of example.matchAll(/(?<![^\n])([A-Z][A-Z0-9_]*)=([^\n]*)(?![^\n])/g)) {
    values.set(m[1] ?? "", m[2] ?? "");
  }
  const out = splitLines(example).map((line) => {
    const key = ENV_LINE.exec(line)?.[1];
    if (key === undefined) return line;
    if (key.endsWith("_KEYS")) {
      const active = values.get(key.slice(0, -"_KEYS".length) + "_ACTIVE_KEY_ID");
      if (!active) throw new ReleaseError(`no active key id for ${key}`);
      return `${key}={"${active}":"${token()}"}`;
    }
    return `${key}=${token()}`;
  });
  return out.join("\n") + "\n";
}

export function setEnv(text: string, key: string, value: string): string {
  const pattern = new RegExp(
    `(?<![^\\n])${key.replace(/[\\^$.*+?()[\]{}|]/g, "\\$&")}=[^\\n]*`,
    "g",
  );
  let replaced = 0;
  const out = text.replace(pattern, () => {
    replaced++;
    return `${key}=${value}`;
  });
  if (replaced !== 1) throw new ReleaseError(`${key} appears ${String(replaced)} times`);
  return out;
}

// A query value as the smoke sends it: unreserved bytes and "/" kept, the
// rest %XX in upper case.
export function urlQuote(text: string): string {
  let out = "";
  for (const byte of Buffer.from(text, "utf8")) {
    const char = String.fromCharCode(byte);
    out += /[A-Za-z0-9_.~/-]/.test(char)
      ? char
      : "%" + byte.toString(16).toUpperCase().padStart(2, "0");
  }
  return out;
}

export function searchHas(results: unknown, id: string): boolean {
  if (!isRecord(results) || !Array.isArray(results.items)) {
    throw new ReleaseError("search results have no items list");
  }
  return results.items.some((item) => {
    if (!isRecord(item)) throw new ReleaseError("search item is not an object");
    return item.documentId === id || item.id === id;
  });
}

function stdinJson(): unknown {
  return JSON.parse(universalNewlines(readFileSync(0, "utf8")));
}

function need(condition: boolean, message: string): void {
  if (!condition) throw new ReleaseError(message);
}

export function main(argv: string[]): number {
  const [command, ...args] = argv;
  const print = (text: string) => process.stdout.write(text + "\n");
  switch (command) {
    case "json-get": {
      let fallback: string | undefined;
      if (args[0] === "--default") {
        fallback = args[1];
        args.splice(0, 2);
      }
      const [json, ...keys] = args;
      need(json !== undefined && keys.length > 0, "json-get needs JSON and a key");
      const value: unknown = JSON.parse(json ?? "");
      if (fallback !== undefined && keys.length === 1 && isRecord(value)) {
        const key = keys[0] ?? "";
        print(Object.hasOwn(value, key) ? display(value[key]) : fallback);
      } else {
        print(display(jsonGet(value, keys)));
      }
      return 0;
    }
    case "index-platforms": {
      const [indexJson, recordPath] = args;
      need(indexJson !== undefined && recordPath !== undefined, "index-platforms needs 2 args");
      const found = indexPlatforms(JSON.parse(indexJson ?? ""));
      const record: unknown = JSON.parse(readText(recordPath ?? ""));
      const expected = isRecord(record) ? record.platforms : undefined;
      need(
        jsonEqual(found, expected),
        `index platforms ${repr(found)} != release record ${repr(expected)}`,
      );
      return 0;
    }
    case "labels": {
      const [labelsJson, version, sha] = args;
      need(sha !== undefined, "labels needs LABELS_JSON VERSION SHA");
      const parsed: unknown = JSON.parse(labelsJson ?? "");
      const labels = isRecord(parsed) ? parsed : {};
      need(
        labels["org.opencontainers.image.version"] === version &&
          labels["org.opencontainers.image.revision"] === sha,
        `image labels do not name ${String(version)}/${String(sha)}: ${repr(labels)}`,
      );
      return 0;
    }
    case "fill-env": {
      const [example, out] = args;
      need(example !== undefined && out !== undefined, "fill-env needs EXAMPLE OUT");
      const text = fillEnv(readText(example ?? ""), () => randomBytes(32).toString("hex"));
      writeFileSync(out ?? "", text, { mode: 0o600 });
      return 0;
    }
    case "set-env": {
      const [path, key, value] = args;
      need(value !== undefined, "set-env needs ENV_FILE KEY VALUE");
      writeFileSync(path ?? "", setEnv(readText(path ?? ""), key ?? "", value ?? ""));
      return 0;
    }
    case "app-service": {
      const apps = appServices(stdinJson());
      need(apps.length === 1, `expected one service publishing 8080, found ${repr(apps)}`);
      print(apps[0] ?? "");
      return 0;
    }
    case "product-services": {
      const [image, app] = args;
      need(app !== undefined, "product-services needs IMAGE APP");
      const config = stdinJson();
      const product = productServices(config, image ?? "");
      need(
        product.includes(app ?? ""),
        `${String(app)} does not run ${String(image)}: ${repr(product)}`,
      );
      const services = isRecord(config) && isRecord(config.services) ? config.services : {};
      need(
        !Object.values(services).some((spec) => isRecord(spec) && truthy(spec.env_file)),
        "a service uses env_file",
      );
      print(["product image services:", product.join(" "), "app:", app].join(" "));
      return 0;
    }
    case "doctor-ok": {
      const report = stdinJson();
      need(isRecord(report) && report.ok === true, `doctor report is not ok: ${repr(report)}`);
      return 0;
    }
    case "uuid":
      print(randomUUID());
      return 0;
    case "url-quote":
      need(args.length === 1, "url-quote needs TEXT");
      print(urlQuote(args[0] ?? ""));
      return 0;
    case "search-has":
      need(args.length === 1, "search-has needs ID");
      return searchHas(stdinJson(), args[0] ?? "") ? 0 : 1;
    case "body-equal": {
      need(args.length === 1, "body-equal needs EXPECTED_JSON");
      const body = stdinJson();
      const expected: unknown = JSON.parse(args[0] ?? "");
      need(isRecord(body) && Object.hasOwn(body, "contentJson"), "body has no contentJson");
      need(
        isRecord(expected) && Object.hasOwn(expected, "contentJson"),
        "expected has no contentJson",
      );
      need(
        jsonEqual(
          (body as Record<string, unknown>).contentJson,
          (expected as Record<string, unknown>).contentJson,
        ),
        `body differs: ${JSON.stringify(body)}`,
      );
      return 0;
    }
    default:
      process.stderr.write("usage: smoke-checks.ts <subcommand> ... (see the file header)\n");
      return 2;
  }
}

if (import.meta.main) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(
      `release-smoke: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    process.exitCode = 1;
  }
}
