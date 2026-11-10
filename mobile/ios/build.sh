#!/usr/bin/env bash
# Builds Excalibur View for the iPad: the Rust library for a real iPad and for
# the Simulator, the PDF engine as a framework, the Xcode project, and then
# either a Simulator build to see it run or an App Store archive.
#
#   mobile/ios/build.sh
#
# Runs on a Mac with Xcode 26 or later (the App Store takes nothing older) and:
#   - Rust, with the iPad targets:
#       rustup target add aarch64-apple-ios aarch64-apple-ios-sim
#   - XcodeGen:  brew install xcodegen
#
# Settings, all optional:
#   BUILD_NUMBER     the build's number, which App Store Connect needs to rise
#                    with every upload. Default: the date and time.
#   PDFIUM_BUILD     the PDF engine build to pack (excaliburct/excalibur-pdfium, chromium/<build>-ios15). Default below.
#   STEPS            "native project simulator" by default. Add "archive" to
#                    make an App Store archive, and "upload" to send it to
#                    App Store Connect for TestFlight.
#
# An archive is signed by Xcode itself, with Apple's automatic signing, which
# needs the team and an App Store Connect API key (App Manager or Admin):
#   TEAM_ID          the ten-character Apple team ID
#   API_KEY_PATH     the .p8 file of the App Store Connect API key
#   API_KEY_ID       its key ID
#   API_ISSUER       its issuer ID
#
# What it leaves behind:
#   mobile/ios/ExcaliburView.xcodeproj      open in Xcode to run it on an iPad
# and in mobile/ios/build:
#   simulator/.../ExcaliburView.app         the Simulator build
#   ExcaliburView.xcarchive, export/*.ipa   with "archive"

set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
work="$here/build"
steps="${STEPS:-native project simulator}"
pdfium_build="${PDFIUM_BUILD:-8066}"
# The PDF engine's two packages, each checked against the SHA-256 its release gave (SHA256SUMS.txt of
# excaliburct/excalibur-pdfium chromium/8066-ios15), so a package can never change underneath the app.
# (A case, not an associative array: macOS's own bash is 3.2, which has none.)
pdfium_sha256() {
    case "$1" in
        device-arm64) echo 0fc9664245afd07a121ce3c78782bd14371c7ae154fd4b377d5711ac9f9a2248 ;;
        simulator-arm64) echo 18a4bbc1f56579b222c20a9db4f810ab5283def1d1701c392a94bd5cc0e5a74d ;;
        *) echo none ;;
    esac
}
min_ios="15.0"   # the oldest iPadOS and iOS the app runs on: Apple's floor for App Store apps built with Xcode 26; our own PDF engine is built for it (excaliburct/excalibur-pdfium)

version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$root/Cargo.toml")"
build_number="${BUILD_NUMBER:-$(date -u +%Y%m%d%H%M)}"
target_dir="${CARGO_TARGET_DIR:-$root/target}"

say() { printf '\n== %s\n' "$*"; }
fail() { printf 'build.sh: %s\n' "$*" >&2; exit 1; }
wants() { [[ " $steps " == *" $1 "* ]]; }

mkdir -p "$work"
say "Excalibur View $version ($build_number) for the iPad"

# ---- the Rust library --------------------------------------------------------

if wants native; then
    export IPHONEOS_DEPLOYMENT_TARGET="$min_ios"
    link_flags=""
    libraries=()
    for target in aarch64-apple-ios aarch64-apple-ios-sim; do
        say "The program, for $target"
        log="$work/rustc-$target.log"
        # --print native-static-libs: the system libraries and frameworks the
        # library needs linked beside it, which a static library cannot carry.
        # Without colour: with it, as GitHub's runners ask for, the last flag
        # came out as "-lm" and a colour code, which no linker can find.
        cargo rustc --color never -p excalibur-view-mobile --release --target "$target" \
            --crate-type staticlib -- --print native-static-libs 2>&1 | tee "$log"
        # And any colour code that got in anyway is taken out.
        esc="$(printf '\033')"
        flags="$(LC_ALL=C sed -e "s/${esc}\[[0-9;]*[A-Za-z]//g" "$log" \
            | sed -n 's/.*native-static-libs: //p' | tail -1)"
        [[ -n "$flags" ]] || fail "rustc did not say what $target links against"
        link_flags="$flags"
        libraries+=(-library "$target_dir/$target/release/libexcalibur_view_mobile.a")
    done
    rm -rf "$work/ExcaliburViewCore.xcframework"
    xcodebuild -create-xcframework "${libraries[@]}" -output "$work/ExcaliburViewCore.xcframework"

    # The version, and what the library links against, for the Xcode project.
    cat > "$work/app.xcconfig" <<EOF
// Written by build.sh. Not kept.
MARKETING_VERSION = $version
CURRENT_PROJECT_VERSION = $build_number
OTHER_LDFLAGS = \$(inherited) $link_flags
EOF

    say "The PDF engine, chromium/$pdfium_build"
    frameworks=()
    for slice in device-arm64 simulator-arm64; do
        archive="$work/pdfium/$pdfium_build-$slice.tgz"
        unpacked="$work/pdfium/$pdfium_build-$slice"
        if [[ ! -f "$archive" ]]; then
            mkdir -p "$work/pdfium"
            curl -fsSL --retry 5 --retry-all-errors --retry-delay 5 \
                "https://github.com/excaliburct/excalibur-pdfium/releases/download/chromium%2F$pdfium_build-ios15/pdfium-ios-$slice.tgz" \
                -o "$archive"
        fi
        if [[ -z "${PDFIUM_BUILD:-}" ]]; then
            got="$(shasum -a 256 "$archive" | cut -d' ' -f1)"
            [[ "$got" == "$(pdfium_sha256 "$slice")" ]] || { rm -f "$archive"; fail "the PDF engine's $slice package isn't the one pinned (SHA-256 $got)"; }
        fi
        rm -rf "$unpacked" && mkdir -p "$unpacked"
        tar -xzf "$archive" -C "$unpacked"
        # The App Store takes a library inside an app only as a framework.
        framework="$unpacked/pdfium.framework"
        mkdir -p "$framework"
        cp "$unpacked/lib/libpdfium.dylib" "$framework/pdfium"
        install_name_tool -id "@rpath/pdfium.framework/pdfium" "$framework/pdfium"
        platform="iPhoneOS"
        [[ "$slice" == simulator-* ]] && platform="iPhoneSimulator"
        cat > "$framework/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key><string>en</string>
	<key>CFBundleExecutable</key><string>pdfium</string>
	<key>CFBundleIdentifier</key><string>com.excaliburct.view.pdfium</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>CFBundleName</key><string>pdfium</string>
	<key>CFBundlePackageType</key><string>FMWK</string>
	<key>CFBundleShortVersionString</key><string>$pdfium_build.0</string>
	<key>CFBundleVersion</key><string>$pdfium_build</string>
	<key>CFBundleSupportedPlatforms</key><array><string>$platform</string></array>
	<key>MinimumOSVersion</key><string>$min_ios</string>
</dict>
</plist>
EOF
        frameworks+=(-framework "$framework")
    done
    rm -rf "$work/pdfium.xcframework"
    xcodebuild -create-xcframework "${frameworks[@]}" -output "$work/pdfium.xcframework"
fi

# ---- the Xcode project ---------------------------------------------------------

if wants project; then
    say "The Xcode project"
    command -v xcodegen > /dev/null || fail "XcodeGen is not installed: brew install xcodegen"
    # Written beside project.yml, not into build/: every path in the spec,
    # and the Info.plist and bridging header settings Xcode reads from the
    # project's own folder, are relative to this folder. Written into build/
    # they all pointed one folder too deep.
    (cd "$here" && xcodegen generate --spec project.yml)
fi

project="$here/ExcaliburView.xcodeproj"

# ---- a Simulator build: does it all compile and link -------------------------

if wants simulator; then
    say "A Simulator build"
    xcodebuild -project "$project" -scheme ExcaliburView -configuration Release \
        -destination "generic/platform=iOS Simulator" \
        -derivedDataPath "$work/simulator" \
        CODE_SIGNING_ALLOWED=NO build
    find "$work/simulator" -name "ExcaliburView.app" -maxdepth 6 -print
fi

# ---- an App Store archive, and TestFlight ----------------------------------------

if wants archive; then
    for needed in TEAM_ID API_KEY_PATH API_KEY_ID API_ISSUER; do
        [[ -n "${!needed:-}" ]] || fail "an archive is signed with Apple's automatic signing, which needs $needed"
    done
    auth=(-allowProvisioningUpdates
          -authenticationKeyPath "$API_KEY_PATH"
          -authenticationKeyID "$API_KEY_ID"
          -authenticationKeyIssuerID "$API_ISSUER")

    say "The App Store archive"
    rm -rf "$work/ExcaliburView.xcarchive"
    xcodebuild -project "$project" -scheme ExcaliburView -configuration Release \
        -destination "generic/platform=iOS" \
        -archivePath "$work/ExcaliburView.xcarchive" \
        DEVELOPMENT_TEAM="$TEAM_ID" CODE_SIGN_STYLE=Automatic \
        "${auth[@]}" archive

    options="$work/ExportOptions.plist"
    cp "$here/ExportOptions.plist" "$options"
    /usr/libexec/PlistBuddy -c "Add :teamID string $TEAM_ID" "$options"
    if wants upload; then
        /usr/libexec/PlistBuddy -c "Set :destination upload" "$options"
        say "Sending it to App Store Connect, for TestFlight"
    else
        say "Exporting the .ipa"
    fi
    rm -rf "$work/export"
    xcodebuild -exportArchive -archivePath "$work/ExcaliburView.xcarchive" \
        -exportOptionsPlist "$options" -exportPath "$work/export" "${auth[@]}"
    ls -l "$work/export" 2>/dev/null || true
fi

say "Done"
