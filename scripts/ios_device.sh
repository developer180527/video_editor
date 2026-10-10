#!/usr/bin/env bash
# Build, sign and run the app on a connected iPad.
#   scripts/ios_device.sh [device-udid]   (default: the first paired iPad)
#
# Signing uses your Apple Development certificate and a provisioning
# profile for the app's bundle id that includes the iPad. If there is none
# yet (or it has expired: a free team's last 7 days), a throwaway Xcode
# project with the same bundle id lets Xcode make one
# (-allowProvisioningUpdates). Set VE_TEAM to your team id if it is not the
# certificate's.
set -euo pipefail
cd "$(dirname "$0")/.."
bundle=$(plutil -extract CFBundleIdentifier raw apps/editor_ios/ios/Info.plist)
udid=${1:-$(xcrun devicectl list devices 2>/dev/null | awk '/iPad/ && /physical/ && (/available/ || /connected/) {for (i=1;i<=NF;i++) if ($i ~ /^[0-9A-F]{8}-[0-9A-F]{16}$/) {print $i; exit}}')}
[[ -n "$udid" ]] || { echo "no paired iPad found (xcrun devicectl list devices)" >&2; exit 1; }
identity=$(security find-identity -v -p codesigning | awk -F'"' '/Apple Development/ {print $2; exit}')
[[ -n "$identity" ]] || { echo "no Apple Development certificate: sign in to Xcode › Settings › Accounts" >&2; exit 1; }
team=${VE_TEAM:-$(security find-certificate -c "$identity" -p | openssl x509 -noout -subject | sed -n 's/.*OU *= *\([A-Z0-9]*\).*/\1/p')}

app=$(scripts/ios_bundle.sh device | tail -1)
plutil -replace CFBundleSupportedPlatforms -json '["iPhoneOS"]' "$app/Info.plist"

# A valid profile for this bundle id that lists the device.
find_profile() {
  local now; now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  for f in "$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles"/*.mobileprovision; do
    [[ -f "$f" ]] || continue
    local p; p=$(security cms -D -i "$f" 2>/dev/null) || continue
    [[ $(plutil -extract Entitlements.application-identifier raw - <<<"$p") == "$team.$bundle" ]] || continue
    [[ $(plutil -extract ExpirationDate raw - <<<"$p") > "$now" ]] || continue
    grep -q "$udid" <<<"$p" || continue
    echo "$f"; return
  done
}
profile=$(find_profile)
if [[ -z "$profile" ]]; then
  echo "making a provisioning profile for $bundle (team $team)…" >&2
  d=target/ios/provision && rm -rf "$d" && mkdir -p "$d/Stub"
  cat > "$d/project.yml" <<YML
name: VEProvision
targets:
  Stub:
    type: application
    platform: iOS
    deploymentTarget: "17.0"
    sources: [Stub]
    settings:
      PRODUCT_BUNDLE_IDENTIFIER: $bundle
      DEVELOPMENT_TEAM: $team
      CODE_SIGN_STYLE: Automatic
      GENERATE_INFOPLIST_FILE: YES
      TARGETED_DEVICE_FAMILY: "2"
YML
  echo 'import SwiftUI
@main struct Stub: App { var body: some Scene { WindowGroup { Text("") } } }' > "$d/Stub/Stub.swift"
  (cd "$d" && xcodegen generate >/dev/null && xcodebuild -project VEProvision.xcodeproj -scheme Stub -destination "id=$udid" \
    -allowProvisioningUpdates -allowProvisioningDeviceRegistration build >/dev/null) || { echo "Xcode could not provision (is the iPad unlocked?)" >&2; exit 1; }
  profile=$(find_profile)
  [[ -n "$profile" ]] || { echo "no profile after provisioning" >&2; exit 1; }
fi

cp "$profile" "$app/embedded.mobileprovision"
ents=$(mktemp -t ve-ents).plist
security cms -D -i "$profile" | plutil -extract Entitlements xml1 -o "$ents" -
codesign --force --sign "$identity" --entitlements "$ents" --generate-entitlement-der "$app"
xcrun devicectl device install app --device "$udid" "$app" >/dev/null
echo "installed on $udid; launching…" >&2
xcrun devicectl device process launch --device "$udid" --terminate-existing "$bundle" >/dev/null
echo "$app"
