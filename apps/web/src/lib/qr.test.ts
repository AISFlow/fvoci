import assert from "node:assert/strict";
import test from "node:test";
import { qrModules } from "./qr.ts";

const URI =
  "otpauth://totp/FVOCI:owner%40example.com?secret=JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP&issuer=FVOCI&algorithm=SHA1&digits=6&period=30";

function darkSet(path: string): Set<string> {
  const cells = new Set<string>();
  for (const match of path.matchAll(/M(\d+) (\d+)h1v1h-1z/g)) {
    const [, x, y] = match;
    assert.ok(x !== undefined && y !== undefined);
    cells.add(`${x},${y}`);
  }
  return cells;
}

await test("qrModules draws a square module grid with the three finder patterns", () => {
  const { size, path } = qrModules(URI);
  // Version v has 17 + 4v modules; a ~120-byte URI at level M needs v >= 6.
  assert.equal((size - 17) % 4, 0);
  assert.ok(size >= 41, `size ${String(size)}`);
  assert.match(path, /^(M\d+ \d+h1v1h-1z)+$/);
  const dark = darkSet(path);
  // Each finder pattern: a dark 7x7 ring with a light ring and a dark 3x3 core.
  for (const [x0, y0] of [
    [0, 0],
    [size - 7, 0],
    [0, size - 7],
  ] as const) {
    for (let i = 0; i < 7; i++) {
      assert.ok(
        dark.has(`${String(x0 + i)},${String(y0)}`) &&
          dark.has(`${String(x0 + i)},${String(y0 + 6)}`),
      );
      assert.ok(
        dark.has(`${String(x0)},${String(y0 + i)}`) &&
          dark.has(`${String(x0 + 6)},${String(y0 + i)}`),
      );
    }
    assert.ok(!dark.has(`${String(x0 + 1)},${String(y0 + 1)}`));
    assert.ok(dark.has(`${String(x0 + 3)},${String(y0 + 3)}`));
  }
});

await test("qrModules is deterministic and depends on the text", () => {
  assert.deepEqual(qrModules(URI), qrModules(URI));
  assert.notEqual(qrModules(URI).path, qrModules(`${URI}x`).path);
  // The path holds only coordinates: no part of the secret is embedded.
  assert.ok(!qrModules(URI).path.includes("JBSW"));
});
