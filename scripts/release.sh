#!/usr/bin/env bash
#
# Build a signed, notarized macOS release of mAIestro.
#
# Why signing matters here (beyond distribution): macOS binds a Keychain item's
# "Always Allow" decision to the app's *designated requirement*. For an ad-hoc /
# unsigned build that requirement is just the binary's cdhash, which changes on
# every rebuild — so the access prompt comes back every launch. A stable
# Developer ID signature anchors the requirement to the certificate instead, so
# "Always Allow" persists across releases.
#
# Requirements:
#   * A "Developer ID Application" certificate in the login keychain. NOT an
#     "Apple Development" cert — that one is for local testing only and the app
#     will not launch on other people's Macs.
#   * Notarization credentials (see below). Without them the build is signed but
#     not notarized, and Gatekeeper will block it on other machines.
#
# Configuration lives in a gitignored .env.release at the repo root (see
# .env.release.example). The script sources it, so you never export by hand.
#
#   APPLE_SIGNING_IDENTITY   e.g. "Developer ID Application: Your Name (CCMY5ZR77Q)"
#
# Notarization — provide EITHER an App Store Connect API key:
#   APPLE_API_ISSUER, APPLE_API_KEY, APPLE_API_KEY_PATH
# OR an Apple ID app-specific password:
#   APPLE_ID, APPLE_PASSWORD, APPLE_TEAM_ID
#
# List available identities with:  security find-identity -v -p codesigning

set -euo pipefail
cd "$(dirname "$0")/.."

# Load signing identity + notarization secrets from the gitignored env file.
if [[ -f .env.release ]]; then
  set -a
  # shellcheck disable=SC1091
  source .env.release
  set +a
fi

if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  echo "error: APPLE_SIGNING_IDENTITY is not set." >&2
  echo "       Copy .env.release.example to .env.release and fill it in." >&2
  exit 1
fi

if ! security find-identity -v -p codesigning | grep -qF "$APPLE_SIGNING_IDENTITY"; then
  echo "error: signing identity not found in keychain: $APPLE_SIGNING_IDENTITY" >&2
  exit 1
fi

case "$APPLE_SIGNING_IDENTITY" in
  "Developer ID Application:"*) ;;
  *)
    echo "warning: '$APPLE_SIGNING_IDENTITY' is not a 'Developer ID Application'" >&2
    echo "         identity; the resulting app may not run on other Macs." >&2
    ;;
esac

# Tauri auto-notarizes when these are present at build time. Warn if absent so a
# silently un-notarized build doesn't slip out.
if [[ -z "${APPLE_API_KEY:-}" && -z "${APPLE_PASSWORD:-}" ]]; then
  echo "warning: no notarization credentials set — build will be signed but NOT" >&2
  echo "         notarized, and Gatekeeper will block it on other machines." >&2
fi

echo "Building signed release as: $APPLE_SIGNING_IDENTITY"
pnpm tauri build

APP="backend/target/release/bundle/macos/mAIestro.app"
echo
echo "Verifying signature…"
codesign --verify --deep --strict --verbose=2 "$APP"
echo "Designated requirement:"
codesign -d -r- "$APP" 2>&1 | sed -n 's/^designated => /  /p'

echo
echo "Gatekeeper assessment:"
spctl --assess --type execute --verbose=4 "$APP" || true

if xcrun stapler validate "$APP" >/dev/null 2>&1; then
  echo "Notarization ticket: stapled ✓"
else
  echo "Notarization ticket: not stapled"
fi

echo
echo "Done. Bundle: $APP"
