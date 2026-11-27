/** Pinned downloads: a file is only used when its SHA-256 is the recorded one. */
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';

export interface Pinned {
  url: string;
  sha256: string;
}

/**
 * Downloads go under `target/` of the repository: it is ignored by git, shared
 * by every checkout step, and on disk (the system temporary directory may be
 * memory).
 */
export const CACHE_DIRECTORY = path.resolve(
  import.meta.dirname,
  '../../../../target/web-fonts/src',
);

const sha256 = (data: Buffer): string => createHash('sha256').update(data).digest('hex');

/** Returns the pinned file, from the cache or the network. */
export async function fetchPinned(cacheName: string, pinned: Pinned): Promise<Buffer> {
  const cached = path.join(CACHE_DIRECTORY, cacheName);
  if (existsSync(cached)) {
    const data = readFileSync(cached);
    if (sha256(data) === pinned.sha256) return data;
  }
  const response = await fetch(pinned.url);
  if (!response.ok) throw new Error(`${pinned.url}: HTTP ${response.status}`);
  const data = Buffer.from(await response.arrayBuffer());
  const actual = sha256(data);
  if (actual !== pinned.sha256) {
    throw new Error(`${pinned.url}: SHA-256 is ${actual}, expected ${pinned.sha256}`);
  }
  mkdirSync(CACHE_DIRECTORY, { recursive: true });
  writeFileSync(cached, data);
  return data;
}
