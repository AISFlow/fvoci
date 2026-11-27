#!/bin/bash
# The entry of the fvoci search image: certificates, the user `fvoci`, then OpenSearch.
#
#   FVOCI_SEARCH_PASSWORD_FILE  a file (a Docker secret) with the password of `fvoci`;
#                               required, read as fvoci reads it (carriage returns and
#                               final newlines dropped), never printed
#   FVOCI_SEARCH_INDEX          the index fvoci writes (fvoci's search_index; `fvoci`):
#                               the only one the user may touch
#
# The certificates are made once, in the volume at config/tls: an authority that signs
# the node's certificate (for the names search and localhost) and an administrator's,
# and is then thrown away. The user, its role and its password are written to the
# security configuration, which becomes the security index at the first start, and are
# set again through the security API, as the administrator, at every start: a new
# password or index takes effect when the service restarts.
set -euo pipefail

home=/usr/share/opensearch
tls=$home/config/tls
security=$home/config/opensearch-security

fail() {
    echo "fvoci-search: $*" >&2
    exit 1
}

file=${FVOCI_SEARCH_PASSWORD_FILE:-}
[ -n "$file" ] || fail "FVOCI_SEARCH_PASSWORD_FILE is not set: it names the file of the password of the user fvoci"
[ -r "$file" ] || fail "FVOCI_SEARCH_PASSWORD_FILE names $file, which cannot be read"
password=$(tr -d '\r' <"$file")
[[ $password =~ [^[:space:]] ]] || fail "FVOCI_SEARCH_PASSWORD_FILE names $file, which is empty or blank"
[[ ! $password =~ [[:cntrl:]] ]] || fail "the password in $file has a control character, which fvoci does not send"
index=${FVOCI_SEARCH_INDEX:-fvoci}
[[ $index =~ ^[a-z0-9][a-z0-9_-]{0,99}$ ]] || fail "FVOCI_SEARCH_INDEX is not a name fvoci takes"

# Signs `name`'s request with the authority in the work directory, with extensions `ext`.
sign() {
    local name=$1 subject=$2
    shift 2
    openssl req -newkey rsa:3072 -nodes -subj "$subject" \
        -keyout "$work/$name-key.pem" -out "$work/$name.csr" || return 1
    printf '%s\n' "$@" >"$work/$name.ext"
    openssl x509 -req -days 3650 -in "$work/$name.csr" -CA "$work/ca.pem" \
        -CAkey "$work/ca-key.pem" -CAcreateserial -extfile "$work/$name.ext" \
        -out "$work/$name.pem" || return 1
}

if [ ! -s "$tls/node.pem" ]; then
    umask 077
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    openssl req -x509 -newkey rsa:3072 -nodes -days 3650 -subj "/CN=fvoci search authority" \
        -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
        -addext "keyUsage=critical,keyCertSign,cRLSign" \
        -keyout "$work/ca-key.pem" -out "$work/ca.pem" ||
        fail "the certificate authority could not be made"
    sign node "/CN=search" "basicConstraints=CA:FALSE" \
        "keyUsage=critical,digitalSignature,keyEncipherment" \
        "extendedKeyUsage=serverAuth,clientAuth" \
        "subjectAltName=DNS:search,DNS:localhost,IP:127.0.0.1" ||
        fail "the certificate of the node could not be made"
    sign admin "/CN=fvoci-admin" "basicConstraints=CA:FALSE" \
        "keyUsage=critical,digitalSignature,keyEncipherment" \
        "extendedKeyUsage=clientAuth" ||
        fail "the administrator's certificate could not be made"
    rm -f "$work/ca-key.pem"
    # Copied beside their places in the volume and then renamed, the node's certificate
    # last: a start that was cut short makes them all again.
    for name in ca.pem admin-key.pem admin.pem node-key.pem node.pem; do
        cp "$work/$name" "$tls/$name.new"
    done
    for name in ca.pem admin-key.pem admin.pem node-key.pem node.pem; do
        mv "$tls/$name.new" "$tls/$name"
    done
    rm -rf "$work"
    trap - EXIT
    umask 022
    echo "fvoci-search: made the certificates of the names search and localhost"
fi
cp "$tls/ca.pem" "$home/ca-out/ca.pem.new"
chmod 644 "$home/ca-out/ca.pem.new"
mv "$home/ca-out/ca.pem.new" "$home/ca-out/ca.pem"

if ! hashed=$(FVOCI_SEARCH_PASSWORD="$password" \
    "$home/plugins/opensearch-security/tools/hash.sh" -env FVOCI_SEARCH_PASSWORD); then
    fail "the password could not be hashed"
fi
hash=$(grep '^\$2' <<<"$hashed" | tail -n 1) || fail "the password could not be hashed"

# What the user may do: its index, and of the cluster what the server's requests need
# (bulk writes, the scrolls of delete-by-query) and the health check reads.
role_json="{\"cluster_permissions\":[\"cluster:monitor/health\",\"indices:data/write/bulk\",\
\"indices:data/read/scroll\",\"indices:data/read/scroll/clear\"],\
\"index_permissions\":[{\"index_patterns\":[\"$index\"],\"allowed_actions\":[\"indices_all\"]}]}"

# Reserved, so that the user cannot change its own password through the account API.
cat >"$security/internal_users.yml" <<YAML
_meta:
  type: "internalusers"
  config_version: 2
fvoci:
  hash: "$hash"
  reserved: true
  description: "The fvoci server"
YAML
cat >"$security/roles.yml" <<YAML
_meta:
  type: "roles"
  config_version: 2
fvoci: $role_json
YAML
cat >"$security/roles_mapping.yml" <<YAML
_meta:
  type: "rolesmapping"
  config_version: 2
fvoci:
  users:
    - "fvoci"
YAML

# The health check signs in as fvoci from a file, so that the password is on no
# command line. The file is in the container's own layer, as the secret is in its
# /run/secrets.
escaped=${password//\\/\\\\}
escaped=${escaped//\"/\\\"}
(
    umask 077
    printf 'user = "fvoci:%s"\ncacert = "%s"\n' "$escaped" "$tls/ca.pem" \
        >"$home/config/healthcheck.curlrc"
)
unset password escaped

# Once the node answers, the user, its role and its mapping are set as the files say,
# as the administrator: the security index keeps those of the first start otherwise.
admin() {
    curl --silent --show-error --max-time 10 --cacert "$tls/ca.pem" \
        --cert "$tls/admin.pem" --key "$tls/admin-key.pem" "$@"
}
apply_security() {
    local tries=0
    until [ "$(admin --output /dev/null --write-out '%{http_code}' \
        https://localhost:9200/_plugins/_security/health 2>/dev/null)" = 200 ]; do
        tries=$((tries + 1))
        [ "$tries" -lt 150 ] || { echo "fvoci-search: the node did not answer; the user fvoci is as the security index had it" >&2; return 1; }
        sleep 2
    done
    local json='content-type: application/json'
    admin --fail --output /dev/null --request PUT --header "$json" --data "$role_json" \
        https://localhost:9200/_plugins/_security/api/roles/fvoci &&
        admin --fail --output /dev/null --request PUT --header "$json" \
            --data '{"users":["fvoci"]}' \
            https://localhost:9200/_plugins/_security/api/rolesmapping/fvoci &&
        admin --fail --output /dev/null --request PATCH --header "$json" \
            --data "[{\"op\":\"replace\",\"path\":\"/hash\",\"value\":\"$hash\"}]" \
            https://localhost:9200/_plugins/_security/api/internalusers/fvoci ||
        { echo "fvoci-search: the user fvoci could not be set" >&2; return 1; }
    echo "fvoci-search: the user fvoci, its role and its password are set"
}
apply_security &

exec "$home/opensearch-docker-entrypoint.sh" "$@"
