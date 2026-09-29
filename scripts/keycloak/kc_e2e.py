#!/usr/bin/env python3
"""Helpers for scripts/keycloak-oidc-e2e.sh (local, opt-in Keycloak OIDC check).

Subcommands:
  render <template> <out>               realm file from the per-run secrets
                                        in the environment
  config <issuer> <out>                 the spec's mode-600 config
  ready <issuer>                        exit 0 once discovery and JWKS answer
  verify <config>                       imported realm/client read back via the
                                        admin API and token endpoint behaviour
  events <config>                       Keycloak event summary (no ids/tokens)
  redact <config>                       stdin to stdout without secrets

Secrets come from the environment or the mode-600 config file and are never
printed; `redact` replaces them and every code/state/token-shaped value.
"""

from __future__ import annotations

import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

REALM = "fvoci-e2e"
CLIENT_ID = "fvoci-e2e"
LABEL = "Keycloak E2E"
USERS = {
    # username: (email, emailVerified, required actions)
    "alice": ("kc-alice@example.com", True, []),
    "bob": ("kc-bob@example.com", True, []),
    "carol": ("kc-carol@example.com", True, []),
    "mallory": ("kc-mallory@example.com", True, []),
    "erin": ("kc-erin@example.com", False, []),
    "tina": ("kc-tina@example.com", True, ["TERMS_AND_CONDITIONS"]),
}
PLACEHOLDER = re.compile(r"@@([A-Z_]+)@@")


def fail(message: str) -> None:
    print(f"keycloak e2e: {message}", file=sys.stderr)
    sys.exit(1)


def http(method: str, url: str, *, form: dict | None = None, token: str | None = None,
         timeout: float = 5.0) -> tuple[int, bytes, dict]:
    data = urllib.parse.urlencode(form).encode() if form is not None else None
    request = urllib.request.Request(url, data=data, method=method)
    if token:
        request.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, response.read(), dict(response.headers)
    except urllib.error.HTTPError as error:
        return error.code, error.read(), dict(error.headers)


def render(template: str, out: str) -> None:
    values = {
        "CLIENT_SECRET": os.environ["KC_E2E_CLIENT_SECRET"],
        **{f"PASSWORD_{name.upper()}": os.environ[f"KC_E2E_PASSWORD_{name.upper()}"] for name in USERS},
    }
    with open(template, encoding="utf-8") as handle:
        text = handle.read()
    missing = sorted({m.group(1) for m in PLACEHOLDER.finditer(text)} - set(values))
    if missing:
        fail(f"template placeholders without a value: {missing}")
    rendered = PLACEHOLDER.sub(lambda m: values[m.group(1)], text)
    json.loads(rendered)
    # Readable by the container's keycloak user; the directory above is 0700.
    fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        handle.write(rendered)


def write_config(issuer: str, out: str) -> None:
    config = {
        "issuer": issuer,
        "realm": REALM,
        "clientId": CLIENT_ID,
        "label": LABEL,
        "keycloakOrigin": issuer.split("/realms/")[0],
        "admin": {"username": "admin", "password": os.environ["KC_BOOTSTRAP_ADMIN_PASSWORD"]},
        "users": {
            name: {"username": name, "email": email,
                   "password": os.environ[f"KC_E2E_PASSWORD_{name.upper()}"]}
            for name, (email, _verified, _actions) in USERS.items()
        },
        "secrets": [os.environ["KC_E2E_CLIENT_SECRET"], os.environ["KC_E2E_WRONG_SECRET"],
                    os.environ["KC_BOOTSTRAP_ADMIN_PASSWORD"]],
    }
    fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        json.dump(config, handle)


def ready(issuer: str) -> None:
    """Discovery answers 200 with exactly this issuer and its JWKS has a signing key."""
    try:
        status, body, _ = http("GET", f"{issuer}/.well-known/openid-configuration", timeout=3)
    except OSError as error:
        fail(f"discovery not reachable ({type(error).__name__})")
    if status != 200:
        fail(f"discovery HTTP {status}")
    doc = json.loads(body)
    if doc.get("issuer") != issuer:
        fail("discovery issuer differs from the configured issuer")
    for key in ("authorization_endpoint", "token_endpoint", "jwks_uri"):
        if not str(doc.get(key, "")).startswith(f"{issuer}/"):
            fail(f"discovery {key} is not under the issuer")
    status, body, _ = http("GET", doc["jwks_uri"], timeout=3)
    if status != 200:
        fail(f"jwks HTTP {status}")
    keys = json.loads(body).get("keys", [])
    if not any(k.get("use") == "sig" and k.get("alg") == "RS256" for k in keys):
        fail("jwks has no RS256 signing key yet")


def load_config(path: str) -> dict:
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def admin_token(config: dict) -> str:
    status, body, _ = http("POST", f"{config['keycloakOrigin']}/realms/master/protocol/openid-connect/token",
                           form={"grant_type": "password", "client_id": "admin-cli",
                                 "username": config["admin"]["username"],
                                 "password": config["admin"]["password"]})
    if status != 200:
        fail(f"admin token HTTP {status}")
    return json.loads(body)["access_token"]


def admin_get(config: dict, token: str, path: str):
    status, body, _ = http("GET", f"{config['keycloakOrigin']}/admin/realms{path}", token=token)
    if status != 200:
        fail(f"admin GET {path} HTTP {status}")
    return json.loads(body)


def verify(path: str) -> None:
    """Reads the imported settings back instead of assuming the import applied."""
    config = load_config(path)
    token = admin_token(config)
    problems: list[str] = []

    def check(name: str, actual, expected) -> None:
        if actual != expected:
            problems.append(f"{name}: {actual!r} != {expected!r}")

    clients = admin_get(config, token, f"/{REALM}/clients?clientId={CLIENT_ID}")
    check("clients named fvoci-e2e", len(clients), 1)
    client = clients[0]
    attributes = client.get("attributes", {})
    summary = {
        "clientId": client.get("clientId"),
        "publicClient": client.get("publicClient"),
        "bearerOnly": client.get("bearerOnly"),
        "clientAuthenticatorType": client.get("clientAuthenticatorType"),
        "standardFlowEnabled": client.get("standardFlowEnabled"),
        "implicitFlowEnabled": client.get("implicitFlowEnabled"),
        "directAccessGrantsEnabled": client.get("directAccessGrantsEnabled"),
        "serviceAccountsEnabled": client.get("serviceAccountsEnabled"),
        "consentRequired": client.get("consentRequired"),
        "fullScopeAllowed": client.get("fullScopeAllowed"),
        "frontchannelLogout": client.get("frontchannelLogout"),
        "redirectUris": client.get("redirectUris"),
        "webOrigins": client.get("webOrigins"),
        "defaultClientScopes": sorted(client.get("defaultClientScopes", [])),
        "optionalClientScopes": sorted(client.get("optionalClientScopes", [])),
        "pkce.code.challenge.method": attributes.get("pkce.code.challenge.method"),
        "oauth2.device.authorization.grant.enabled": attributes.get("oauth2.device.authorization.grant.enabled"),
        "oidc.ciba.grant.enabled": attributes.get("oidc.ciba.grant.enabled"),
        "standard.token.exchange.enabled": attributes.get("standard.token.exchange.enabled"),
        "post.logout.redirect.uris": attributes.get("post.logout.redirect.uris"),
    }
    expected = {
        "clientId": CLIENT_ID, "publicClient": False, "bearerOnly": False,
        "clientAuthenticatorType": "client-secret", "standardFlowEnabled": True,
        "implicitFlowEnabled": False, "directAccessGrantsEnabled": False,
        "serviceAccountsEnabled": False, "consentRequired": False, "fullScopeAllowed": False,
        "frontchannelLogout": False,
        # The server's port is chosen at start: the spec registers the exact
        # callback URI of each server before its first flow.
        "redirectUris": [], "webOrigins": [],
        "defaultClientScopes": ["basic"], "optionalClientScopes": ["email", "profile"],
        "pkce.code.challenge.method": "S256",
        "oauth2.device.authorization.grant.enabled": "false", "oidc.ciba.grant.enabled": "false",
        "standard.token.exchange.enabled": "false",
    }
    for key, value in expected.items():
        check(f"client {key}", summary[key], value)

    realm = admin_get(config, token, f"/{REALM}")
    realm_summary = {k: realm.get(k) for k in (
        "realm", "enabled", "sslRequired", "registrationAllowed", "resetPasswordAllowed",
        "verifyEmail", "eventsEnabled")}
    check("realm sslRequired", realm_summary["sslRequired"], "external")
    check("realm registrationAllowed", realm_summary["registrationAllowed"], False)
    users = admin_get(config, token, f"/{REALM}/users?max=100")
    user_summary = sorted(
        (u["username"], u.get("email"), u.get("emailVerified"), sorted(u.get("requiredActions", [])))
        for u in users)
    check("realm users", user_summary, sorted(
        (name, email, verified, actions) for name, (email, verified, actions) in USERS.items()))
    actions = {a["alias"]: a.get("enabled") for a in
               admin_get(config, token, f"/{REALM}/authentication/required-actions")}
    check("TERMS_AND_CONDITIONS enabled", actions.get("TERMS_AND_CONDITIONS"), True)
    master_users = sorted(u["username"] for u in admin_get(config, token, "/master/users?max=100"))
    check("master realm users", master_users, ["admin"])

    # Token endpoint behaviour with the real secret: grants other than the
    # authorization code are refused for this client.
    token_url = f"{config['issuer']}/protocol/openid-connect/token"
    secret = config["secrets"][0]
    refusals = {}
    for grant, extra in (("password", {"username": "alice", "password": config["users"]["alice"]["password"]}),
                         ("client_credentials", {})):
        status, body, _ = http("POST", token_url, form={
            "grant_type": grant, "client_id": CLIENT_ID, "client_secret": secret, "scope": "openid", **extra})
        error = json.loads(body or b"{}").get("error")
        refusals[grant] = {"status": status, "error": error}
        if status < 400 or "access_token" in json.loads(body or b"{}"):
            problems.append(f"{grant} grant was not refused")

    report = {"client": summary, "realm": realm_summary, "users": user_summary,
              "requiredActions": actions, "masterRealmUsers": master_users,
              "tokenEndpointRefusals": refusals}
    print(json.dumps(report, indent=2))
    if problems:
        fail("imported settings differ:\n  " + "\n  ".join(problems))


def events(path: str) -> None:
    config = load_config(path)
    token = admin_token(config)
    raw = admin_get(config, token, f"/{REALM}/events?max=1000")
    keep = ("grant_type", "client_auth_method", "auth_method", "redirect_uri", "response_type",
            "scope", "username", "custom_required_action", "reason")
    rows = []
    for event in sorted(raw, key=lambda e: e.get("time", 0)):
        details = event.get("details") or {}
        rows.append({
            "type": event.get("type"),
            "clientId": event.get("clientId"),
            "error": event.get("error"),
            "details": {k: details[k] for k in keep if k in details},
        })
    counts: dict[str, int] = {}
    for row in rows:
        key = f"{row['type']} {row['error'] or ''}".strip()
        counts[key] = counts.get(key, 0) + 1
    print(json.dumps({"counts": dict(sorted(counts.items())), "events": rows}, indent=2))


URL_PARAMS = re.compile(
    r"\b((?:code|state|session_state|session_code|client_data|tab_id|nonce|code_challenge"
    r"|code_verifier|id_token|access_token|refresh_token|id_token_hint|invitation|mfa)=)"
    r"[^&\s\"'<>]+")
COOKIES = re.compile(
    r"\b(fvoci_session|fvoci_oidc_state|KEYCLOAK_[A-Z_]+|AUTH_SESSION_ID[A-Z_]*|KC_RESTART"
    r"|KC_AUTH_SESSION_HASH|KC_STATE_CHECKER)=[^;\s\"']+")
JWT = re.compile(r"eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]*")
INVITE = re.compile(r"/invite/[A-Za-z0-9_-]{16,}")
POSTGRES = re.compile(r"postgres(?:ql)?://\S+")
BEARER = re.compile(r"Bearer\s+\S+")


def redact(path: str) -> None:
    secrets: list[str] = []
    if path and os.path.exists(path):
        config = load_config(path)
        secrets = [*config.get("secrets", []), *(u["password"] for u in config["users"].values())]
    secrets = sorted({s for s in secrets if len(s) >= 8}, key=len, reverse=True)
    for line in sys.stdin:
        for secret in secrets:
            line = line.replace(secret, "<redacted-secret>")
        line = URL_PARAMS.sub(r"\1<redacted>", line)
        line = COOKIES.sub(r"\1=<redacted>", line)
        line = JWT.sub("<redacted-jwt>", line)
        line = INVITE.sub("/invite/<redacted>", line)
        line = POSTGRES.sub("postgres://<redacted>", line)
        line = BEARER.sub("Bearer <redacted>", line)
        sys.stdout.write(line)
        sys.stdout.flush()


def main() -> None:
    if len(sys.argv) < 2:
        fail(__doc__ or "usage")
    command, args = sys.argv[1], sys.argv[2:]
    if command == "render" and len(args) == 2:
        render(*args)
    elif command == "config" and len(args) == 2:
        write_config(*args)
    elif command == "ready" and len(args) == 1:
        ready(args[0])
    elif command == "verify" and len(args) == 1:
        verify(args[0])
    elif command == "events" and len(args) == 1:
        events(args[0])
    elif command == "redact" and len(args) <= 1:
        redact(args[0] if args else "")
    else:
        fail(f"unknown command or arguments: {command}")


if __name__ == "__main__":
    main()
