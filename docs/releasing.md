# Releasing Jilo Studio

Jilo Studio ships for macOS on Apple silicon (macOS 15 or later). A release is built, signed and
notarized on a Mac, then published as a GitHub Release of
[guloff/jilo-studio](https://github.com/guloff/jilo-studio/releases), where the app's updates come
from.

User-facing names say **Jilo Studio**. Release files are lowercase:
`jilo-studio-<version>-macos-aarch64.dmg` (the app) and `jilo-studio-cli-<version>-macos-aarch64.zip`
(the headless CLI). The crates and binaries inside keep FilmCraft's names (`filmcraft`,
`filmcraft-cli`), so changes can flow between the two projects.

## Cutting a release

1. Set the version (one source of truth, `[workspace.package] version`):

   ```sh
   cargo xtask version set 0.6.0        # pre-releases: 0.6.0-rc.1
   ```

2. Run the quality gates: `cargo fmt --all -- --check`, `cargo xtask ci`, and the tests of the whole
   workspace.

3. Build, sign and notarize:

   ```sh
   MACOS_SIGN_IDENTITY="Developer ID Application: <name> (<team id>)" \
   MACOS_NOTARY_PROFILE=<notarytool keychain profile> \
   bash packaging/macos/package.sh
   ```

   Without `MACOS_SIGN_IDENTITY` the script signs ad-hoc (for local testing only; Gatekeeper rejects
   it on other Macs). With an identity and a notary profile it notarizes and staples the app and the
   DMG, and notarizes the CLI zip.

4. Check the result: `packaging/macos/smoke.sh target/macos-package/bin` (import → sequence → export
   → ffprobe) and
   `python3 -I packaging/macos/check_symbols.py "target/macos-package/Jilo Studio.app/Contents/MacOS/Jilo Studio"`
   (no framework symbol newer than macOS 15).

5. Publish:

   ```sh
   gh release create v0.6.0 --repo guloff/jilo-studio --title "Jilo Studio 0.6.0" --notes-file NOTES.md \
     dist/release/jilo-studio-0.6.0-macos-aarch64.dmg dist/release/jilo-studio-cli-0.6.0-macos-aarch64.zip
   ```

   Pre-releases (`-rc.N`) get `--prerelease`.

## How the app finds a release

Help ▸ Check for Updates… (`app.checkUpdates`, `crates/engine/src/updates.rs`) asks GitHub for the
latest release of `guloff/jilo-studio` (`/releases/latest`: drafts and pre-releases are not offered)
and compares its tag, `vX.Y.Z`, with the running version. A newer release shows its notes (the
release body, as plain text) and a Download button that opens the release page. The app checks
only when asked and downloads nothing itself; so the tag must carry the version number, and the notes
are what users read before they download.

## What gets built

`packaging/macos/package.sh` builds `filmcraft` and `filmcraft-cli` in release mode for
`aarch64-apple-darwin` with `MACOSX_DEPLOYMENT_TARGET=15.0`, then:

- assembles `Jilo Studio.app` (`packaging/macos/Info.plist.in`, the Jilo icon, the licences in
  `Contents/Resources/Licenses`) and signs it with the hardened runtime and
  `packaging/macos/entitlements.plist` (microphone, for voice-over);
- puts it on a drag-to-Applications DMG with the window layout of `packaging/macos/dmg/`;
- zips the CLI with its README and licences.

The packaging scripts for Windows, Linux and FreeBSD under `packaging/` come from FilmCraft; this
fork does not build or test them.
