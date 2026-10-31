#!/bin/sh
# A stable local identity reduces repeated screen-recording and Keychain authorization across builds.
set -eu
JIE_IDENTITY='Jie Local Development'
umask 077
JIE_TMP=$(mktemp -d "${TMPDIR:-/tmp}/jie-signing.XXXXXX")
trap 'rm -rf "$JIE_TMP"' EXIT HUP INT TERM
JIE_KEYCHAIN=$(security default-keychain -d user | tr -d '"' | sed 's/^ *//')
if security find-certificate -c "$JIE_IDENTITY" -p > "$JIE_TMP/cert.pem" 2>/dev/null; then
    security add-trusted-cert -r trustRoot -p codeSign -k "$JIE_KEYCHAIN" "$JIE_TMP/cert.pem"
    printf '%s\n' "$JIE_IDENTITY is ready"
    exit 0
fi
cat > "$JIE_TMP/openssl.cnf" <<'CNF'
[req]
distinguished_name = subject
x509_extensions = codesigning
prompt = no
[subject]
CN = Jie Local Development
[codesigning]
basicConstraints = critical,CA:FALSE
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CNF
openssl req -new -newkey rsa:2048 -nodes -x509 -days 3650 \
    -config "$JIE_TMP/openssl.cnf" -keyout "$JIE_TMP/key.pem" -out "$JIE_TMP/cert.pem" 2> "$JIE_TMP/openssl.log"
JIE_PASSWORD=$(openssl rand -hex 24)
export JIE_PASSWORD
openssl pkcs12 -export -inkey "$JIE_TMP/key.pem" -in "$JIE_TMP/cert.pem" \
    -name "$JIE_IDENTITY" -out "$JIE_TMP/identity.p12" -passout env:JIE_PASSWORD
JIE_KEYCHAIN=$(security default-keychain -d user | tr -d '"' | sed 's/^ *//')
security import "$JIE_TMP/identity.p12" -k "$JIE_KEYCHAIN" -P "$JIE_PASSWORD" -T /usr/bin/codesign
unset JIE_PASSWORD
security add-trusted-cert -r trustRoot -p codeSign -k "$JIE_KEYCHAIN" "$JIE_TMP/cert.pem"
printf '%s\n' "Created local signing identity: $JIE_IDENTITY"
