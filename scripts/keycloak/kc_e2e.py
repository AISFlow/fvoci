#!/usr/bin/env python3
"""Helpers for scripts/keycloak-oidc-e2e.sh (local, opt-in Keycloak OIDC check).

Subcommands:
  render <template> <out>               realm file from the per-run secrets
                                        in the environment
  render-sso <template> <out-dir>       the two workspace SSO realm files
  config <issuer> <out>                 the spec's mode-600 config
  sso-config <keycloak-origin> <out>    the Rust workspace SSO test's config
  ready <issuer>...                     exit 0 once discovery and JWKS answer
  verify <config> [<sso-config>]        imported realms/clients read back via
                                        the admin API and token endpoint
                                        behaviour
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
# Workspace SSO (opt-in Rust test): two realms, i.e. two issuers, one client
# and one user each.
SSO_REALMS = {
    "A": {"realm": "fvoci-e2e-ws-a", "username": "ws-a", "email": "kc-ws-a@example.com"},
    "B": {"realm": "fvoci-e2e-ws-b", "username": "ws-b", "email": "kc-ws-b@example.com"},
}
SSO_CLIENT_ID = "fvoci-ws"
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


def render_file(template: str, out: str, values: dict) -> None:
    with open(template, encoding="utf-8") as handle:
        text = handle.read()
    missing = sorted({m.group(1) for m in PLACEHOLDER.finditer(text)} - set(values))
    if missing:
        fail(f"template placeholders without a value: {missing}")
    rendered = PLACEHOLDER.sub(lambda m: values[m.group(1)], text)
    json.loads(rendered)
    # Readable by the container's keycloak user; the run directory is 0700.
    fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        handle.write(rendered)


def render(template: str, out: str) -> None:
    render_file(template, out, {
        "CLIENT_SECRET": os.environ["KC_E2E_CLIENT_SECRET"],
        **{f"PASSWORD_{name.upper()}": os.environ[f"KC_E2E_PASSWORD_{name.upper()}"] for name in USERS},
    })


def sso_secret(key: str, what: str) -> str:
    return os.environ[f"KC_E2E_SSO_{key}_{what}"]


def render_sso(template: str, out_dir: str) -> None:
    for key, realm in SSO_REALMS.items():
        render_file(template, os.path.join(out_dir, f"{realm['realm']}-realm.json"), {
            "REALM": realm["realm"],
            "CLIENT_SECRET": sso_secret(key, "CLIENT_SECRET"),
            "USERNAME": realm["username"],
            "EMAIL": realm["email"],
            "PASSWORD": sso_secret(key, "PASSWORD"),
        })


def write_sso_config(keycloak_origin: str, out: str) -> None:
    config = {
        "keycloakOrigin": keycloak_origin,
        "admin": {"username": "admin", "password": os.environ["KC_BOOTSTRAP_ADMIN_PASSWORD"]},
        "realms": [
            {
                "realm": realm["realm"],
                "issuer": f"{keycloak_origin}/realms/{realm['realm']}",
                "clientId": SSO_CLIENT_ID,
                "clientSecret": sso_secret(key, "CLIENT_SECRET"),
                "username": realm["username"],
                "password": sso_secret(key, "PASSWORD"),
                "email": realm["email"],
            }
            for key, realm in SSO_REALMS.items()
        ],
    }
    fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as handle:
        json.dump(config, handle)


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
        # FVOCI accounts the spec creates (password sign-in).
        "fvoci": {"ownerPassword": os.environ["KC_E2E_FVOCI_OWNER_PASSWORD"],
                  "memberPassword": os.environ["KC_E2E_FVOCI_MEMBER_PASSWORD"]},
        "secrets": [os.environ["KC_E2E_CLIENT_SECRET"], os.environ["KC_E2E_WRONG_SECRET"],
                    os.environ["KC_BOOTSTRAP_ADMIN_PASSWORD"],
                    os.environ["KC_E2E_FVOCI_OWNER_PASSWORD"], os.environ["KC_E2E_FVOCI_MEMBER_PASSWORD"],
                    *(sso_secret(key, what) for key in SSO_REALMS
                      for what in ("CLIENT_SECRET", "PASSWORD"))],
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


def client_summary(config: dict, token: str, realm: str, client_id: str, problems: list[str]) -> dict:
    clients = admin_get(config, token, f"/{realm}/clients?clientId={client_id}")
    if len(clients) != 1:
        problems.append(f"{realm}: {len(clients)} clients named {client_id}")
        return {}
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
        "clientId": client_id, "publicClient": False, "bearerOnly": False,
        "clientAuthenticatorType": "client-secret", "standardFlowEnabled": True,
        "implicitFlowEnabled": False, "directAccessGrantsEnabled": False,
        "serviceAccountsEnabled": False, "consentRequired": False, "fullScopeAllowed": False,
        "frontchannelLogout": False,
        # The server's port (or workspace id) is known only later: the spec and
        # the Rust test register the exact callback URI before the first flow.
        "redirectUris": [], "webOrigins": [],
        "defaultClientScopes": ["basic"], "optionalClientScopes": ["email", "profile"],
        "pkce.code.challenge.method": "S256",
        "oauth2.device.authorization.grant.enabled": "false", "oidc.ciba.grant.enabled": "false",
        "standard.token.exchange.enabled": "false",
    }
    for key, value in expected.items():
        if summary[key] != value:
            problems.append(f"{realm} client {key}: {summary[key]!r} != {value!r}")
    return summary


def realm_summary(config: dict, token: str, realm: str, users: list, problems: list[str]) -> dict:
    rep = admin_get(config, token, f"/{realm}")
    summary = {k: rep.get(k) for k in (
        "realm", "enabled", "sslRequired", "registrationAllowed", "resetPasswordAllowed",
        "verifyEmail", "eventsEnabled", "eventsListeners")}
    for key, value in (("sslRequired", "external"), ("registrationAllowed", False),
                       ("eventsEnabled", True), ("eventsListeners", [])):
        if summary[key] != value:
            problems.append(f"{realm} {key}: {summary[key]!r} != {value!r}")
    actual = sorted(
        [u["username"], u.get("email"), u.get("emailVerified"), sorted(u.get("requiredActions", []))]
        for u in admin_get(config, token, f"/{realm}/users?max=100"))
    if actual != sorted(users):
        problems.append(f"{realm} users: {actual!r} != {sorted(users)!r}")
    summary["users"] = actual
    return summary


def grant_refusals(issuer: str, client_id: str, secret: str, username: str, password: str,
                   problems: list[str]) -> dict:
    """Grants other than the authorization code are refused, with the real secret."""
    token_url = f"{issuer}/protocol/openid-connect/token"
    refusals = {}
    for grant, extra in (("password", {"username": username, "password": password}),
                         ("client_credentials", {})):
        status, body, _ = http("POST", token_url, form={
            "grant_type": grant, "client_id": client_id, "client_secret": secret, "scope": "openid",
            **extra})
        answer = json.loads(body or b"{}")
        refusals[grant] = {"status": status, "error": answer.get("error")}
        if status < 400 or "access_token" in answer:
            problems.append(f"{issuer}: {grant} grant was not refused")
    return refusals


def verify(path: str, sso_path: str | None = None) -> None:
    """Reads the imported settings back instead of assuming the import applied."""
    config = load_config(path)
    token = admin_token(config)
    problems: list[str] = []
    users = [[name, email, verified, actions] for name, (email, verified, actions) in USERS.items()]
    report = {
        "realm": realm_summary(config, token, REALM, users, problems),
        "client": client_summary(config, token, REALM, CLIENT_ID, problems),
        "tokenEndpointRefusals": grant_refusals(
            config["issuer"], CLIENT_ID, config["secrets"][0], "alice",
            config["users"]["alice"]["password"], problems),
    }
    actions = {a["alias"]: a.get("enabled") for a in
               admin_get(config, token, f"/{REALM}/authentication/required-actions")}
    if actions.get("TERMS_AND_CONDITIONS") is not True:
        problems.append("TERMS_AND_CONDITIONS is not enabled")
    report["requiredActions"] = actions
    master_users = sorted(u["username"] for u in admin_get(config, token, "/master/users?max=100"))
    if master_users != ["admin"]:
        problems.append(f"master realm users: {master_users!r}")
    report["masterRealmUsers"] = master_users
    if sso_path:
        report["workspaceSsoRealms"] = {}
        for realm in load_config(sso_path)["realms"]:
            name = realm["realm"]
            report["workspaceSsoRealms"][name] = {
                "realm": realm_summary(config, token, name,
                                       [[realm["username"], realm["email"], True, []]], problems),
                "client": client_summary(config, token, name, realm["clientId"], problems),
                "tokenEndpointRefusals": grant_refusals(
                    realm["issuer"], realm["clientId"], realm["clientSecret"], realm["username"],
                    realm["password"], problems),
            }
    print(json.dumps(report, indent=2))
    if problems:
        fail("imported settings differ:\n  " + "\n  ".join(problems))


def events(path: str, realms: list[str]) -> None:
    config = load_config(path)
    token = admin_token(config)
    keep = ("grant_type", "client_auth_method", "auth_method", "redirect_uri", "response_type",
            "scope", "username", "custom_required_action", "reason")
    report = {}
    for realm in realms or [REALM]:
        raw = admin_get(config, token, f"/{realm}/events?max=1000")
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
        report[realm] = {"counts": dict(sorted(counts.items())), "events": rows}
    print(json.dumps(report, indent=2))


URL_PARAMS = re.compile(
    r"\b((?:code|state|session_state|session_code|client_data|tab_id|nonce|code_challenge"
    r"|code_verifier|id_token|access_token|refresh_token|id_token_hint|invitation|mfa)=)"
    r"[^&\s\"'<>]+")
# `key="value"` (Keycloak's event log) and `"key": "value"` (JSON). The
# ambiguous names keep lower_snake_case values (problem codes such as
# "origin_mismatch", the pool's state="open"); anything else is redacted.
AMBIGUOUS_KEYS = "code|state|nonce|session_state"
ALWAYS_KEYS = ("code_id|session_code|client_data|tab_id|auth_session_[a-z_]+|userSessionId"
               "|sessionId|session_id|code_challenge|code_verifier|access_token|refresh_token"
               "|id_token|id_token_hint|invitation|mfa")
QUOTED = re.compile(
    rf"(\b(?P<key>{AMBIGUOUS_KEYS}|{ALWAYS_KEYS})=\"|\"(?P<jkey>{AMBIGUOUS_KEYS}|{ALWAYS_KEYS})\"\s*:\s*\")"
    r"(?P<value>[^\"]*)\"")
AMBIGUOUS = re.compile(rf"^(?:{AMBIGUOUS_KEYS})$")
PLAIN_WORD = re.compile(r"^[a-z][a-z_]*$")
COOKIES = re.compile(
    r"\b(fvoci_session|fvoci_oidc_state|KEYCLOAK_[A-Z_]+|AUTH_SESSION_ID[A-Z_]*|KC_RESTART"
    r"|KC_AUTH_SESSION_HASH|KC_STATE_CHECKER)=[^;\s\"']+")
JWT = re.compile(r"eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]*")
INVITE = re.compile(r"/invite/[A-Za-z0-9_-]{16,}")
POSTGRES = re.compile(r"postgres(?:ql)?://\S+")
BEARER = re.compile(r"\bBearer\s+\S+")
BASIC = re.compile(r"\bBasic\s+[A-Za-z0-9+/]{8,}={0,2}")


def quoted(match: re.Match) -> str:
    key = match.group("key") or match.group("jkey")
    value = match.group("value")
    if AMBIGUOUS.match(key) and PLAIN_WORD.match(value):
        return match.group(0)
    return f"{match.group(1)}<redacted>\""


def redact_line(line: str, secrets: list[str]) -> str:
    for secret in secrets:
        line = line.replace(secret, "<redacted-secret>")
    line = QUOTED.sub(quoted, line)
    line = URL_PARAMS.sub(r"\1<redacted>", line)
    line = COOKIES.sub(r"\1=<redacted>", line)
    line = JWT.sub("<redacted-jwt>", line)
    line = INVITE.sub("/invite/<redacted>", line)
    line = POSTGRES.sub("postgres://<redacted>", line)
    line = BEARER.sub("Bearer <redacted>", line)
    return BASIC.sub("Basic <redacted>", line)


def redact(path: str) -> None:
    secrets: list[str] = []
    if path and os.path.exists(path):
        config = load_config(path)
        secrets = [*config.get("secrets", []), *(u["password"] for u in config["users"].values())]
    secrets = sorted({s for s in secrets if len(s) >= 8}, key=len, reverse=True)
    for line in sys.stdin:
        sys.stdout.write(redact_line(line, secrets))
        sys.stdout.flush()


def main() -> None:
    if len(sys.argv) < 2:
        fail(__doc__ or "usage")
    command, args = sys.argv[1], sys.argv[2:]
    if command == "render" and len(args) == 2:
        render(*args)
    elif command == "render-sso" and len(args) == 2:
        render_sso(*args)
    elif command == "config" and len(args) == 2:
        write_config(*args)
    elif command == "sso-config" and len(args) == 2:
        write_sso_config(*args)
    elif command == "ready" and args:
        for issuer in args:
            ready(issuer)
    elif command == "verify" and len(args) in (1, 2):
        verify(*args)
    elif command == "events" and args:
        events(args[0], args[1:])
    elif command == "redact" and len(args) <= 1:
        redact(args[0] if args else "")
    else:
        fail(f"unknown command or arguments: {command}")


if __name__ == "__main__":
    main()
