#!/usr/bin/env python3
"""Registry and GitHub release calls for the release workflow (docs/RELEASING.md).

Decisions use HTTP status codes and JSON fields, never error-message wording.
Standard library only.

  release-api.py registry-digest --image ghcr.io/aisflow/fvoci --tag 0.y.z
      prints the manifest digest of a tag, or nothing when it does not exist;
      exit 4 when the registry refuses to answer (401/403).
  release-api.py push-index --image IMAGE --amd64 sha256:... --arm64 sha256:...
      pushes a two-platform index BY DIGEST (no tag) and prints its digest.
  release-api.py describe --image IMAGE --digest sha256:...
      prints index_digest/amd64_digest/arm64_digest lines for a two-platform
      linux index (fails on anything else).
  release-api.py tag --image IMAGE --digest sha256:... --tag 0.y.z [--floating]
      points a tag at an existing index. Without --floating the tag is
      immutable: an existing tag with another digest fails. With --floating the
      tag moves. Either way the pushed bytes are the digest's own bytes.
  release-api.py release-state --tag v0.y.z
      prints {"state": "none"|"published"|"draft", "assets": [...]}.

Registry credentials: REGISTRY_USER and REGISTRY_PASSWORD (anonymous when
unset). GitHub: GH_TOKEN and GITHUB_REPOSITORY (GITHUB_API_URL optional).
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

DIGEST = re.compile(r"sha256:[0-9a-f]{64}")
TAG = re.compile(r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}")
DOCKER_MANIFEST = "application/vnd.docker.distribution.manifest.v2+json"
DOCKER_LIST = "application/vnd.docker.distribution.manifest.list.v2+json"
OCI_MANIFEST = "application/vnd.oci.image.manifest.v1+json"
OCI_INDEX = "application/vnd.oci.image.index.v1+json"
ACCEPT_IMAGE = ", ".join((OCI_MANIFEST, DOCKER_MANIFEST))
ACCEPT_INDEX = ", ".join((OCI_INDEX, DOCKER_LIST))
EXIT_UNREADABLE = 4


class HttpStatus(Exception):
    def __init__(self, status: int, headers: dict, body: bytes) -> None:
        super().__init__(f"HTTP {status}")
        self.status = status
        self.headers = headers
        self.body = body


def die(message: str, code: int = 1) -> None:
    print(f"release-api: {message}", file=sys.stderr)
    sys.exit(code)


def request(url: str, method: str = "GET", headers: dict | None = None, data: bytes | None = None):
    req = urllib.request.Request(url, method=method, headers=headers or {}, data=data)
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            return resp.status, {k.lower(): v for k, v in resp.headers.items()}, resp.read()
    except urllib.error.HTTPError as err:
        raise HttpStatus(err.code, {k.lower(): v for k, v in err.headers.items()}, err.read()) from None


class Registry:
    def __init__(self, image: str, actions: str) -> None:
        if not re.fullmatch(r"[a-z0-9.-]+(/[a-z0-9._-]+)+", image):
            die(f"--image {image!r} must be a lowercase registry/repository without tag")
        self.host, self.name = image.split("/", 1)
        self.base = f"https://{self.host}/v2/{self.name}"
        self.token = self._token(actions)

    def _token(self, actions: str) -> str | None:
        """Bearer token from the realm the registry announces (distribution spec)."""
        try:
            request(f"https://{self.host}/v2/")
            return None
        except HttpStatus as err:
            if err.status != 401:
                die(f"https://{self.host}/v2/ answered HTTP {err.status}")
            challenge = err.headers.get("www-authenticate", "")
        params = dict(re.findall(r'(\w+)="([^"]*)"', challenge))
        if not challenge.lower().startswith("bearer ") or "realm" not in params:
            die(f"unsupported registry challenge: {challenge!r}")
        query = {"scope": f"repository:{self.name}:{actions}"}
        if "service" in params:
            query["service"] = params["service"]
        headers = {}
        user, password = os.environ.get("REGISTRY_USER"), os.environ.get("REGISTRY_PASSWORD")
        if user and password:
            headers["Authorization"] = "Basic " + base64.b64encode(f"{user}:{password}".encode()).decode()
        try:
            _, _, body = request(params["realm"] + "?" + urllib.parse.urlencode(query), headers=headers)
        except HttpStatus as err:
            if err.status in (401, 403):
                die(f"registry token for {self.name} refused (HTTP {err.status})", EXIT_UNREADABLE)
            raise
        payload = json.loads(body)
        return payload.get("token") or payload.get("access_token")

    def _headers(self, extra: dict | None = None) -> dict:
        headers = dict(extra or {})
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        return headers

    def manifest(self, reference: str, accept: str) -> tuple[str, str, bytes] | None:
        """(digest, media type, bytes) of a manifest, None on 404."""
        try:
            _, headers, body = request(f"{self.base}/manifests/{reference}", headers=self._headers({"Accept": accept}))
        except HttpStatus as err:
            if err.status == 404:
                return None
            if err.status in (401, 403):
                die(f"{self.name}:{reference} not readable (HTTP {err.status})", EXIT_UNREADABLE)
            die(f"{self.name}:{reference}: HTTP {err.status}")
        digest = "sha256:" + hashlib.sha256(body).hexdigest()
        announced = headers.get("docker-content-digest")
        if announced and announced != digest:
            die(f"{self.name}:{reference}: registry digest {announced} != content digest {digest}")
        if DIGEST.fullmatch(reference) and reference != digest:
            die(f"{self.name}@{reference}: content digest is {digest}")
        media = json.loads(body).get("mediaType") or headers.get("content-type", "")
        return digest, media.split(";")[0].strip(), body

    def put(self, reference: str, media: str, body: bytes) -> str:
        digest = "sha256:" + hashlib.sha256(body).hexdigest()
        try:
            status, headers, _ = request(
                f"{self.base}/manifests/{reference}", "PUT", self._headers({"Content-Type": media}), body
            )
        except HttpStatus as err:
            die(f"push {self.name}:{reference}: HTTP {err.status} {err.body[:300]!r}")
        announced = headers.get("docker-content-digest")
        if status != 201 or (announced and announced != digest):
            die(f"push {self.name}:{reference}: HTTP {status}, digest {announced} != {digest}")
        return digest


def registry_digest(args) -> None:
    if not TAG.fullmatch(args.tag):
        die(f"invalid tag {args.tag!r}", 2)
    found = Registry(args.image, "pull").manifest(args.tag, ACCEPT_INDEX)
    print(found[0] if found else "")


def push_index(args) -> None:
    reg = Registry(args.image, "pull,push")
    manifests = []
    for arch, digest in (("amd64", args.amd64), ("arm64", args.arm64)):
        if not DIGEST.fullmatch(digest):
            die(f"--{arch} must be sha256:<64 hex>", 2)
        found = reg.manifest(digest, ACCEPT_IMAGE)
        if found is None:
            die(f"{args.image}@{digest} ({arch}) does not exist")
        _, media, body = found
        if media not in (OCI_MANIFEST, DOCKER_MANIFEST):
            die(f"{args.image}@{digest} ({arch}) is {media!r}, not a single-platform image manifest")
        manifests.append({"mediaType": media, "digest": digest, "size": len(body),
                          "platform": {"architecture": arch, "os": "linux"}})
    docker = all(m["mediaType"] == DOCKER_MANIFEST for m in manifests)
    index = {"schemaVersion": 2, "mediaType": DOCKER_LIST if docker else OCI_INDEX, "manifests": manifests}
    body = json.dumps(index, separators=(",", ":")).encode()
    digest = "sha256:" + hashlib.sha256(body).hexdigest()
    print(reg.put(digest, index["mediaType"], body))


def describe(args) -> None:
    """Per-platform digests of a two-platform linux index, as shell outputs."""
    if not DIGEST.fullmatch(args.digest):
        die("--digest must be sha256:<64 hex>", 2)
    found = Registry(args.image, "pull").manifest(args.digest, ACCEPT_INDEX)
    if found is None:
        die(f"{args.image}@{args.digest} does not exist")
    _, media, body = found
    index = json.loads(body)
    if media not in (OCI_INDEX, DOCKER_LIST):
        die(f"{args.image}@{args.digest} is {media!r}, not an index")
    platforms = {}
    for entry in index.get("manifests", []):
        platform = entry.get("platform", {})
        key = f"{platform.get('os')}/{platform.get('architecture')}"
        if key in platforms or key not in ("linux/amd64", "linux/arm64"):
            die(f"{args.image}@{args.digest}: unexpected or repeated platform {key}")
        platforms[key] = entry["digest"]
    if set(platforms) != {"linux/amd64", "linux/arm64"}:
        die(f"{args.image}@{args.digest}: platforms {sorted(platforms)} are not linux/amd64 and linux/arm64")
    print(f"index_digest={args.digest}")
    print(f"amd64_digest={platforms['linux/amd64']}")
    print(f"arm64_digest={platforms['linux/arm64']}")


def tag(args) -> None:
    if not DIGEST.fullmatch(args.digest) or not TAG.fullmatch(args.tag):
        die("--digest must be sha256:<64 hex> and --tag a registry tag", 2)
    reg = Registry(args.image, "pull,push")
    found = reg.manifest(args.digest, ACCEPT_INDEX)
    if found is None:
        die(f"{args.image}@{args.digest} does not exist")
    _, media, body = found
    current = reg.manifest(args.tag, ACCEPT_INDEX)
    if current and current[0] == args.digest:
        print(f"{args.image}:{args.tag} already {args.digest}", file=sys.stderr)
        return
    if current and not args.floating:
        die(f"{args.image}:{args.tag} is {current[0]}, not {args.digest}; immutable tags are never moved")
    reg.put(args.tag, media, body)
    after = reg.manifest(args.tag, ACCEPT_INDEX)
    if not after or after[0] != args.digest:
        die(f"{args.image}:{args.tag} reads back as {after and after[0]}, not {args.digest}")
    print(f"{args.image}:{args.tag} -> {args.digest}" + (f" (was {current[0]})" if current else ""), file=sys.stderr)


def release_state(args) -> None:
    repo, token = os.environ.get("GITHUB_REPOSITORY", ""), os.environ.get("GH_TOKEN", "")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo) or not token:
        die("GITHUB_REPOSITORY and GH_TOKEN are required", 2)
    api = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
    headers = {"Authorization": f"Bearer {token}", "Accept": "application/vnd.github+json",
               "X-GitHub-Api-Version": "2022-11-28"}
    releases = []
    # Drafts have no tag lookup; list every release (drafts are listed for
    # tokens with contents: write, which is the token that could create one).
    for page in range(1, 51):
        try:
            _, _, body = request(f"{api}/repos/{repo}/releases?per_page=100&page={page}", headers=headers)
        except HttpStatus as err:
            die(f"listing releases of {repo}: HTTP {err.status}")
        batch = json.loads(body)
        releases += [r for r in batch if r.get("tag_name") == args.tag]
        if len(batch) < 100:
            break
    else:
        die("more than 5000 releases; refusing to guess")
    if len(releases) > 1:
        die(f"{len(releases)} releases name {args.tag}; delete the extra ones by hand")
    if not releases:
        print(json.dumps({"state": "none", "assets": []}))
        return
    release = releases[0]
    print(json.dumps({"state": "draft" if release.get("draft") else "published",
                      "assets": sorted(a["name"] for a in release.get("assets", []))}))


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("registry-digest")
    p.add_argument("--image", required=True)
    p.add_argument("--tag", required=True)
    p.set_defaults(func=registry_digest)
    p = sub.add_parser("push-index")
    p.add_argument("--image", required=True)
    p.add_argument("--amd64", required=True)
    p.add_argument("--arm64", required=True)
    p.set_defaults(func=push_index)
    p = sub.add_parser("describe")
    p.add_argument("--image", required=True)
    p.add_argument("--digest", required=True)
    p.set_defaults(func=describe)
    p = sub.add_parser("tag")
    p.add_argument("--image", required=True)
    p.add_argument("--digest", required=True)
    p.add_argument("--tag", required=True)
    p.add_argument("--floating", action="store_true")
    p.set_defaults(func=tag)
    p = sub.add_parser("release-state")
    p.add_argument("--tag", required=True)
    p.set_defaults(func=release_state)
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
