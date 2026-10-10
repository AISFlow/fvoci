// Redacted registry and GitHub response bodies in the shapes ghcr.io and the
// REST API return (OCI image-spec manifests, distribution-spec token answer,
// GitHub "list releases"). Digests of config/layers are placeholders; the
// tokens are fixed fake strings, never real credentials.
import { DOCKER_MANIFEST, OCI_MANIFEST } from "./registry.ts";

export const IMAGE = "ghcr.io/aisflow/fvoci";
export const REPOSITORY_NAME = "aisflow/fvoci";
export const REGISTRY_BASE = "https://ghcr.io";
export const GITHUB_API = "https://api.github.test";
export const REGISTRY_TOKEN = "redacted-registry-token";
export const FAKE_USER = "fixture-user";
export const FAKE_PASSWORD = "fixture-password-not-a-secret";
export const FAKE_GH_TOKEN = "fixture-gh-token-not-a-secret";

const placeholder = (n: number) => "sha256:" + n.toString(16).padStart(64, "0");

/** A single-platform image manifest as a registry stores it (exact bytes). */
export function platformManifest(arch: string, mediaType = OCI_MANIFEST): string {
  const docker = mediaType === DOCKER_MANIFEST;
  const n = arch === "amd64" ? 1 : 2;
  return JSON.stringify(
    {
      schemaVersion: 2,
      mediaType,
      config: {
        mediaType: docker
          ? "application/vnd.docker.container.image.v1+json"
          : "application/vnd.oci.image.config.v1+json",
        digest: placeholder(n * 16),
        size: 7023,
      },
      layers: [
        {
          mediaType: docker
            ? "application/vnd.docker.image.rootfs.diff.tar.gzip"
            : "application/vnd.oci.image.layer.v1.tar+gzip",
          digest: placeholder(n * 16 + 1),
          size: 32654,
        },
      ],
      annotations: { "org.opencontainers.image.version": "0.1.0" },
    },
    null,
    2,
  );
}

/** A GitHub "list releases" entry, trimmed to the fields release-state reads plus a few neighbours. */
export function release(
  tag: string,
  draft: boolean,
  assets: string[],
  id = 1,
): Record<string, unknown> {
  return {
    id,
    tag_name: tag,
    name: `FVOCI ${tag.slice(1)} (trial)`,
    draft,
    prerelease: true,
    assets: assets.map((name, i) => ({ id: id * 100 + i, name, state: "uploaded", size: 100 + i })),
  };
}

export const RELEASE_ASSETS = [
  "compose.yml",
  "env.example",
  "INSTALL.md",
  "SHA256SUMS",
  "release.json",
  "RELEASE-NOTES.md",
];
