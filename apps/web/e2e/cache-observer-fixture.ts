import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import type { BrowserContext } from "@playwright/test";

/** Overlay only this wrapper run's private static copy. Rust serves every
 * document, asset, API, SSE and collaboration response normally. */
export function buildCacheObserverFixture() {
  const staticDir = process.env.FVOCI_STATIC_DIR;
  const resultDir = process.env.FVOCI_E2E_RESULT_DIR;
  if (
    !staticDir ||
    !resultDir ||
    realpathSync(staticDir) !== path.join(realpathSync(resultDir), "static")
  ) {
    throw new Error("Cache fixture requires the wrapper-owned result/static directory");
  }
  const dist = mkdtempSync(path.join(tmpdir(), "fvoci-cache-observer-"));
  const sha256 = (body: string | Buffer) => createHash("sha256").update(body).digest("hex");
  try {
    execFileSync(
      "bun",
      ["--bun", "x", "--no-install", "vite", "build", "--mode=task-cache-e2e", `--outDir=${dist}`],
      {
        cwd: path.resolve(import.meta.dirname, ".."),
        env: { ...process.env, NODE_ENV: "production" },
        stdio: "pipe",
      },
    );
    // Server CSP is computed at startup from inline shell content. The fixture
    // may change external chunk URLs; everything else must remain identical.
    const original = readFileSync(path.join(staticDir, "index.html"), "utf8");
    const fixture = readFileSync(path.join(dist, "index.html"), "utf8");
    const withoutExternalJs = (html: string) =>
      html.replace(/((?:src|href)=")\/assets\/[^"/]+\.js"/g, '$1/EXTERNAL-JS"');
    if (withoutExternalJs(original) !== withoutExternalJs(fixture)) {
      throw new Error("Fixture shell changed inline CSP content or production styles");
    }
    const files = readdirSync(dist, { recursive: true, withFileTypes: true }).filter((entry) =>
      entry.isFile(),
    );
    const manifest = files.map((entry) => {
      const relative = path.relative(dist, path.join(entry.parentPath, entry.name));
      return { path: relative, sha256: sha256(readFileSync(path.join(dist, relative))) };
    });
    cpSync(dist, staticDir, { recursive: true });
    for (const entry of manifest) {
      if (sha256(readFileSync(path.join(staticDir, entry.path))) !== entry.sha256) {
        throw new Error(`Fixture overlay mismatch: ${entry.path}`);
      }
    }
    const byPath = new Map(manifest.map((entry) => [`/${entry.path}`, entry.sha256]));
    const observed: Promise<{
      path: string;
      sha256: string;
      securityHeaders: Record<string, string>;
    }>[] = [];
    return {
      install(context: BrowserContext) {
        context.on("response", (response) => {
          const pathname = new URL(response.url()).pathname;
          if (!pathname.startsWith("/assets/") || response.status() !== 200) return;
          observed.push(
            response.body().then((body) => {
              const hash = sha256(body);
              if (byPath.get(pathname) !== hash)
                throw new Error(`Served fixture asset mismatch: ${pathname}`);
              const securityHeaders = Object.fromEntries(
                Object.entries(response.headers()).filter(([key]) =>
                  [
                    "content-security-policy",
                    "x-content-type-options",
                    "referrer-policy",
                    "permissions-policy",
                    "cross-origin-resource-policy",
                    "cross-origin-opener-policy",
                  ].includes(key),
                ),
              );
              return { path: pathname, sha256: hash, securityHeaders };
            }),
          );
        });
      },
      async evidence() {
        return {
          mode: "task-cache-e2e",
          shellUnchangedExceptExternalJs: true,
          manifest,
          served: await Promise.all(observed),
        };
      },
      dispose() {
        rmSync(dist, { recursive: true, force: true });
      },
    };
  } catch (error) {
    rmSync(dist, { recursive: true, force: true });
    throw error;
  }
}
