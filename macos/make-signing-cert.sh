#!/bin/zsh
# One-time setup of the fixed signing identity "OpenCleaner Code Signing" (self-signed).
#
# Why: macOS ties permissions such as Full Disk Access to the app's signature. Ad-hoc builds get a
# new signature every time, so users would have to grant access again after each update. A fixed
# certificate keeps it. It does NOT remove the Gatekeeper warning; that needs Developer ID + notarization.
#
# Usage:
#   ./make-signing-cert.sh                  create the certificate in ~/.opencleaner-signing (once)
#   ./make-signing-cert.sh --import         also add it to the login keychain, for signed local builds
#   ./make-signing-cert.sh --secrets OWNER/REPO
#                                           upload MACOS_CERT_P12, MACOS_CERT_PASSWORD and MACOS_SIGN_ID
#                                           with gh (logged in as an account that admins the repo)
#
# Keep ~/.opencleaner-signing private and backed up: whoever has the key can sign apps macOS treats
# as OpenCleaner, and losing it means users grant permissions again after the next update.
set -euo pipefail

NAME="OpenCleaner Code Signing"
DIR=${OPENCLEANER_SIGNING_DIR:-$HOME/.opencleaner-signing}
P12=$DIR/cert.p12
PASS_FILE=$DIR/password

if [[ ! -f $P12 ]]; then
  mkdir -p $DIR && chmod 700 $DIR
  openssl rand -base64 24 | tr -d '\n' > $PASS_FILE
  chmod 600 $PASS_FILE
  openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -subj "/CN=$NAME" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning" \
    -keyout $DIR/key.pem -out $DIR/cert.pem 2>/dev/null
  # -legacy: macOS `security import` cannot read the newer OpenSSL 3 p12 encryption
  openssl pkcs12 -export -legacy -name "$NAME" -inkey $DIR/key.pem -in $DIR/cert.pem \
    -out $P12 -passout "file:$PASS_FILE"
  chmod 600 $DIR/*
  echo "created $P12"
else
  echo "using existing $P12"
fi

for arg in "$@"; do
  case $arg in
    --import)
      security import $P12 -k "$HOME/Library/Keychains/login.keychain-db" -P "$(cat $PASS_FILE)" -T /usr/bin/codesign
      echo "imported into the login keychain; build signed with: SIGN_ID=\"$NAME\" ./build.sh"
      ;;
    --secrets) ;;
    */*)
      base64 -i $P12 | gh secret set MACOS_CERT_P12 --repo "$arg"
      gh secret set MACOS_CERT_PASSWORD --repo "$arg" < $PASS_FILE
      gh secret set MACOS_SIGN_ID --repo "$arg" --body "$NAME"
      echo "secrets set on $arg"
      ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done
