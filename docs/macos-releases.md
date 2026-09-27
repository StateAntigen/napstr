# macOS builds and releases

Napstr and Napstrfy share `scripts/macos-release.mjs` for local and CI builds.
The release sequence follows [LNbits PR #4175](https://github.com/lnbits/lnbits/pull/4175),
using Tauri to build these applications. Local commands never create a tag,
publish a release, or upload installers to GitHub.

## Prerequisites

Use a native Intel or Apple Silicon Mac with Node.js 24, Rust/Cargo, Xcode
command-line tools, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).
Accept the Xcode license and ensure `xcrun notarytool --version` works for signed
builds. Run `npm ci` in the application directory first. Cross-compilation and
Rosetta are deliberately unsupported by this helper; build each architecture
on a corresponding Mac or GitHub runner.

The helper uses Cargo from `PATH`, then checks `$CARGO_HOME/bin` (normally
`~/.cargo/bin`). If the Cargo launcher is missing, it asks `rustup` for the active
toolchain and makes its Cargo and compiler available to build subprocesses.
This preserves [Rustup's toolchain selection](https://rust-lang.github.io/rustup/overrides.html),
including project overrides and `RUSTUP_TOOLCHAIN`, without editing shell settings.
If neither Cargo nor Rustup is installed, install Rust from https://rustup.rs and
reopen your terminal before retrying.

## Local commands

From the repository root:

```sh
npm ci
npm run macos-build
npm run macos-build:signed
```

For Napstrfy, either run those same commands from `android/`, or use:

```sh
npm --prefix android ci
npm --prefix android run macos-build
npm --prefix android run macos-build:signed
```

`macos-build` creates an ad-hoc community build without Apple credentials.
Downloaded community builds may require **System Settings → Privacy & Security
→ Open Anyway** after the first blocked launch. `macos-build:signed` requires
Developer ID credentials and creates a signed, notarized, stapled release build.
There is no automatic fallback to a community build when signing fails.

Outputs are under the selected application's `src-tauri/target/macos-release/`:

```text
signed/Napstr_0.1.0_arm64.dmg
signed/Napstr_0.1.0_arm64.dmg.sha256
unsigned/Napstr_0.1.0_arm64.dmg
unsigned/Napstr_0.1.0_arm64.dmg.sha256
```

Napstrfy filenames start with `Napstrfy`; Intel filenames use `x86_64`.
Use `-- --tag v0.1.5-rc1` to set the packaged version without editing tracked
version files. The root release workflow also synchronizes source versions
using its existing `set-release-version.mjs` step.
Apple bundle metadata uses the numeric version (`0.1.5` for `v0.1.5-rc1`);
the DMG filename retains the full release version.

## Signing credentials

Signing requires an Apple Developer Program membership and a valid **Developer
ID Application** certificate with its private key. Export that identity from
Keychain Access as a password-protected `.p12`. An Apple Development or Developer
ID Installer certificate is unsuitable. Export exactly one current identity for
the expected team. See [Tauri's macOS signing guide](https://v2.tauri.app/distribute/sign/macos/).

Use `.env.macos-release.example` as the template for a private file named
`.env.macos-release` at the repository root. Both applications read this same
file. It is Git-ignored and must have mode `0600`:

```sh
chmod 600 .env.macos-release
```

| Variable | Value |
| --- | --- |
| `BUILD_CERTIFICATE_BASE64` | Base64-encoded P12 containing the certificate and private key. |
| `P12_PASSWORD` | Password used to export the P12. |
| `KEYCHAIN_PASSWORD` | A separate password for the disposable build keychain, not the login keychain password. |
| `APPLE_ID` | Apple account email associated with the developer team. |
| `APPLE_TEAM_ID` | Ten-character team ID matching the certificate. |
| `APPLE_APP_SPECIFIC_PASSWORD` | An app-specific password from account.apple.com, not the account login password. |

Store the base64 value on one line. The parser accepts `NAME=value` and quoted
values; it does not evaluate shell code, expand variables, or support multiline
values or inline comments. Never source this file or commit real credentials.
The helper excludes signing credentials from frontend, Cargo, Tauri, and Tor
subprocess environments, and redacts credential-bearing diagnostics.

## Build and verification sequence

1. Validate the requested mode, credentials, and native architecture. Build the
   `.app` with Tauri signing disabled so all final signing happens in one place.
   Napstr downloads and verifies its pinned Tor Expert Bundle.
2. In Napstr's app, make Tor's bundled library references relative to their
   loader. Reject unresolved non-system libraries. This avoids depending on
   `DYLD_LIBRARY_PATH` or granting a hardened-runtime exception for it.
3. For signed builds, import the P12 into a unique temporary keychain. Select
   exactly one valid Developer ID Application identity for the configured team
   and validate notarization credentials. Preserve the existing keychain search
   list; never replace the user's default keychain.
4. Sign native code and nested bundles from the inside out, then sign the app.
   Developer ID signatures use hardened runtime and secure timestamps. Neither
   application needs executable-memory or library-validation exceptions.
5. Verify native architectures and signatures. For signed builds, independently
   verify the team, Developer ID requirement, timestamps, hardened runtime, and
   absence of extra entitlements. Submit an app ZIP to Apple, require status
   `Accepted`, then staple and validate the app's ticket.
6. Copy the finalized app with `ditto`, preserving its ticket and symlinks, into
   a DMG payload with an Applications shortcut. Create an uncompressed HFS+
   image and convert it to the final compressed DMG. For signed builds, sign,
   independently notarize, staple, and validate that exact final DMG.
7. Verify the image, mount it read-only, repeat app signature and architecture
   checks, and run the bundled Tor executable without DYLD overrides. Signed
   builds also require Gatekeeper assessments and valid app/DMG tickets.
8. Detach the image, retrying briefly if macOS still reports it busy after the
   verification checks. Restore and verify the original keychain search list, and
   delete temporary signing material. Only then write and recheck the SHA-256
   checksum of the final stapled image. Failures remove the attempted DMG and
   checksum; cleanup failures retain a recovery journal.

Notarization submits once per artifact, prints the submission ID, and waits up
to 30 minutes. A timeout or rejection fails the build and reports Apple's
diagnostics. The first submission of a new app can take longer; inspect the
submission in Apple's service before retrying. There is no silent skip.

For recovery after an uncatchable termination or restart:

```sh
npm run macos-build -- --cleanup
npm --prefix android run macos-build -- --cleanup
```

The journal lives at the application's `src-tauri/target/macos-release-state.json`
and contains paths and the original keychain search list, never credentials.
Do not run local macOS builds concurrently: keychain search lists belong to the
current user, including across the two applications.

## GitHub Actions

Configure the six names above as repository Actions secrets accessible to
`lnbits/napstr`. The same names are used by LNbits, but secrets restricted to
that repository are not automatically available here.

`release.yml` handles both Napstr release builds and optional macOS test builds
on `macos-15` (Apple Silicon) and `macos-15-intel`. A `v*` tag or a manual run with
`release_tag` set runs the full Napstr desktop and Napstrfy Android release process.
`napstrfy-desktop.yml` runs signed Napstrfy desktop builds for tags and
manual runs; its pull-request builds use community signatures without secrets.
Manual Napstrfy runs on a branch produce workflow artifacts; tagged runs attach
them to the draft release. Existing `v*` release triggers are preserved.

For Napstr testing without creating a release, prefix a PR title with `[build]`,
for example `[build] Fix audio playback`. The `macos_test` job in `release.yml` builds
Intel and Apple Silicon installers when that PR opens, reopens, receives new
commits, or has its title edited. Adding the prefix to an existing PR starts a
build; removing it skips subsequent builds. Description-only edits do not build.
There is no changed-file filter: the title flag controls whether a PR builds.
PRs skip the release jobs, including Linux, Windows, Android, and release uploads.

Same-repository PRs use the six Apple signing secrets to create signed installers.
Fork PRs, Dependabot-authored PRs, and runs triggered by Dependabot use ad-hoc
signatures without Apple credentials. Those artifacts end in `-unsigned` and may
show Gatekeeper's unverified-developer warning. PRs without `[build]` skip the
Napstr macOS build job.

You can also use **Actions → Build Napstr desktop and Napstrfy Android installers
→ Run workflow**, select a branch, and leave `release_tag` empty for a signed
macOS test build. Setting `release_tag` instead runs the full release process.
Signed PR and manual test runs produce these artifacts:

- `Napstr-macOS-Intel-signed`
- `Napstr-macOS-Apple-Silicon-signed`

Download the artifact for your Mac from the completed run, extract it, and open
the enclosed DMG. It includes a SHA-256 checksum. The version comes from the
selected branch's Tauri configuration; no release tag is required or created.
The workflow must first exist on the default branch to enable manual runs; see
[GitHub's manual-run documentation](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow).

All signed workflows invoke the same npm commands with `--ci`, which reads credentials
from the environment instead of a local file. Cleanup runs with `always()`
before upload. DMGs and checksum files are uploaded only after successful
verification and cleanup. Release publication remains a separate action.

Before publishing the first release, test downloaded installers on both an Intel
Mac and an Apple Silicon Mac, including app startup, Tor connection, file
transfer, audio playback, and Napstrfy pairing. The helper's signature and Tor
checks do not replace this interactive application testing.
