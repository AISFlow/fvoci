// Independent v1 ENCRYPTION_KEYS manifest compatibility oracle.
//
// Operational backup and restore use the shared Rust fvoci-migrate modes; this
// oracle keeps differential vectors and negative checks for that format.
//
//   bun tools/oracle/encryption-keys.ts manifest           print the manifest entry (JSON)
//   bun tools/oracle/encryption-keys.ts check ENTRY_FILE   exit 1 if the env keyring cannot
//                                                          open what the backed-up keyring sealed
//   bun tools/oracle/encryption-keys.ts self-test
//
// The keyring is read from ENCRYPTION_KEYS and ENCRYPTION_ACTIVE_KEY_ID (both
// or neither, as the server requires). Keys never leave the environment and
// are never printed.
import { readFileSync } from "node:fs";

import {
  checkEntry,
  KeyringError,
  manifestEntry,
  parseKeyring,
  pyJson,
} from "./encryption-keyring.ts";
import { selfTest } from "./encryption-keys-self-test.ts";

export const USAGE = `usage:
  encryption-keys.ts manifest            print the manifest entry (JSON)
  encryption-keys.ts check ENTRY_FILE    exit 1 if the env keyring cannot open
                                         what the backed-up keyring sealed
  encryption-keys.ts self-test

The keyring is read from the ENCRYPTION_KEYS and ENCRYPTION_ACTIVE_KEY_ID
environment variables (both or neither, as the server requires).
`;

export interface Io {
  env: Record<string, string | undefined>;
  out: (text: string) => void;
  err: (text: string) => void;
  readBytes: (path: string) => Uint8Array;
}

function envKeyring(env: Io["env"]) {
  return parseKeyring(env.ENCRYPTION_KEYS, env.ENCRYPTION_ACTIVE_KEY_ID);
}

function readEntry(path: string, io: Io): unknown {
  let bytes: Uint8Array;
  try {
    bytes = io.readBytes(path);
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code ?? "unreadable";
    throw new EntryFileError(`cannot read ${path}: ${code}`);
  }
  let text: string;
  try {
    // A BOM is kept so JSON.parse refuses it, as v1 did.
    text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch {
    throw new EntryFileError(`${path} is not valid UTF-8`);
  }
  try {
    return JSON.parse(text) as unknown;
  } catch {
    throw new EntryFileError(`${path} is not valid JSON`);
  }
}

class EntryFileError extends Error {}

export function main(argv: string[], io: Io): number {
  try {
    if (argv.length === 1 && argv[0] === "manifest") {
      io.out(`${pyJson(manifestEntry(envKeyring(io.env)))}\n`);
      return 0;
    }
    if (argv.length === 2 && argv[0] === "check") {
      // The entry file is read before the env keyring, as in v1.
      const entry = readEntry(argv[1] ?? "", io);
      const problems = checkEntry(entry, envKeyring(io.env));
      for (const problem of problems) io.err(`${problem}\n`);
      return problems.length ? 1 : 0;
    }
    if (argv.length === 1 && argv[0] === "self-test") {
      selfTest();
      io.out("encryption_keys self-test ok\n");
      return 0;
    }
  } catch (error) {
    if (error instanceof KeyringError || error instanceof EntryFileError) {
      io.err(`${error.message}\n`);
      return 1;
    }
    throw error;
  }
  io.err(USAGE);
  return 2;
}

if (import.meta.main) {
  process.exitCode = main(process.argv.slice(2), {
    env: process.env,
    out: (text) => process.stdout.write(text),
    err: (text) => process.stderr.write(text),
    readBytes: (path) => readFileSync(path),
  });
}
