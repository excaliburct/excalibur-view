#!/usr/bin/env bash
#
# The same thing as "Release Excalibur View.bat", for a Mac or a Linux box.
#
# Whichever machine ends up holding hyperview-signing.key is the machine that
# runs this. Nothing about it assumes Windows: the script it calls finds the
# key, the token and the notes by name rather than by a path somebody typed in
# once.
#
# With the release key on a hardware key (a YubiKey) instead, run it twice:
# `--prepare --slot SLOT` before signing on the computer the key is plugged
# into, and `--finish REQUEST --signatures FILE` after. docs/RELEASING.md has
# the steps.

cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
exec python3 tools/release_now.py "$@"
