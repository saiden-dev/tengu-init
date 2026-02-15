# Tengu Init - Build & Release Tasks

# Default recipe
default:
    @just --list

# Build release binary for current platform
build:
    cargo build --release

# Build for Apple Silicon
build-mac:
    cargo build --release --target aarch64-apple-darwin

# Sign macOS binary (requires Developer ID certificate)
sign: build-mac
    #!/usr/bin/env bash
    set -euo pipefail
    BINARY="target/aarch64-apple-darwin/release/tengu-init"
    IDENTITY=$(security find-identity -v -p codesigning | grep "Developer ID Application" | head -1 | awk -F'"' '{print $2}')
    if [[ -z "$IDENTITY" ]]; then
        echo "Error: No Developer ID Application certificate found"
        exit 1
    fi
    echo "Signing with: $IDENTITY"
    codesign --force --options runtime --sign "$IDENTITY" "$BINARY"
    codesign -dv --verbose=2 "$BINARY"
    echo "✓ Signed successfully"

# Notarize macOS binary (requires stored credentials)
notarize: sign
    #!/usr/bin/env bash
    set -euo pipefail
    BINARY="target/aarch64-apple-darwin/release/tengu-init"
    ZIP="target/tengu-init-apple-silicon.zip"

    echo "Creating zip for notarization..."
    zip -j "$ZIP" "$BINARY"

    echo "Submitting for notarization..."
    xcrun notarytool submit "$ZIP" --keychain-profile "tengu-notary" --wait

    rm "$ZIP"
    echo "✓ Notarized successfully"

# Store notarization credentials (run once)
notary-setup:
    #!/usr/bin/env bash
    echo "This will store credentials in your keychain."
    echo "You need:"
    echo "  - Apple ID email"
    echo "  - Team ID (from developer.apple.com)"
    echo "  - App-specific password (from appleid.apple.com)"
    echo ""
    xcrun notarytool store-credentials "tengu-notary" \
        --apple-id "adam.ladachowski@gmail.com" \
        --team-id "$(read -p 'Team ID: ' tid; echo $tid)"

# Full release: build, sign, notarize, copy to release name
release: notarize
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
    BINARY="target/aarch64-apple-darwin/release/tengu-init"
    OUTPUT="tengu-init-apple-silicon"

    cp "$BINARY" "$OUTPUT"
    echo "✓ Release ready: $OUTPUT (v$VERSION)"
    ls -lh "$OUTPUT"

# Verify signature
verify:
    codesign -dv --verbose=4 target/aarch64-apple-darwin/release/tengu-init
