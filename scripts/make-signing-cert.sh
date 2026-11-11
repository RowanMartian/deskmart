#!/bin/bash
# One-time setup, run on your Mac: makes a private code-signing certificate for Deskmart and copies it
# (base64) to the clipboard, for the GitHub secret DESKMART_SIGNING_P12. With it, every GitHub build is
# signed by the same certificate, so macOS keeps Full Disk Access and App Management across updates.
# The certificate never leaves your Mac and GitHub. Don't commit the files this makes.
set -euo pipefail
DIR="$(mktemp -d)"
trap 'rm -rf "$DIR"' EXIT
PASS="$(LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 32)"
cat > "$DIR/cert.cnf" <<'CNF'
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = Deskmart Self-Signed
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
subjectKeyIdentifier = hash
CNF
openssl req -x509 -newkey rsa:2048 -nodes -days 7300 -keyout "$DIR/key.pem" -out "$DIR/cert.pem" -config "$DIR/cert.cnf" 2>/dev/null
LEGACY=""
openssl version | grep -q "OpenSSL 3" && LEGACY="-legacy"
openssl pkcs12 -export $LEGACY -inkey "$DIR/key.pem" -in "$DIR/cert.pem" -name "Deskmart Self-Signed" -out "$DIR/deskmart.p12" -passout "pass:$PASS"
base64 -i "$DIR/deskmart.p12" | tr -d '\n' | pbcopy
cat <<MSG

Done. The certificate is on your clipboard.

On GitHub, open your Deskmart repository → Settings → Secrets and variables → Actions → New repository secret:

  1. Name: DESKMART_SIGNING_P12        Secret: paste (⌘V)
  2. Name: DESKMART_SIGNING_PASSWORD   Secret: $PASS

Then re-run the "Build Mac app" workflow (Actions tab). Install that build by hand once and turn on
Full Disk Access for it; from then on, updates keep it.
MSG
