# Napstrfy

Napstr's companion player for Android, Windows, Linux, and macOS.

For desktop pairing, paste the code from Napstr's **Napstrfy → Pair without a camera**.

## Desktop

From `android/`, with the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) installed:

```sh
npm ci
npm run desktop          # Develop
npm run bundle:windows   # EXE (on Windows)
npm run macos-build      # Ad-hoc community DMG (on macOS)
npm run macos-build:signed # Signed, notarized, and stapled DMG (on macOS)
npm run appimage:build   # AppImage (Linux with Docker)
```

On NixOS, run from the repository root:

```sh
nix develop --command npm --prefix android run desktop
```

The development shell includes the GStreamer plugins WebKit needs for audio
playback. After changing the shell dependencies, stop the running client and
run this command again to load the updated environment.

Pushing a `v*` tag attaches installers and SHA-256 checksum files to a draft release.
Release macOS builds require Apple signing credentials; pull-request builds use
ad-hoc signatures. `bundle:macos` remains an alias for `macos-build`.
Both macOS commands build the current Mac's native architecture, share the root
`.env.macos-release` for signed local builds, and never upload anything. See
[macOS release setup](../docs/macos-releases.md).

## Android requirements

- Node.js
- Rust and Cargo
- JDK 17
- Android SDK, platform tools and build tools
- Android NDK
- `adb`

Install the Rust Android targets:

```sh
rustup default stable
rustup target add \
  aarch64-linux-android \
  armv7-linux-androideabi \
  i686-linux-android \
  x86_64-linux-android
```

## First setup

```sh
cd android
npm ci
npm run android:init
```

Enable USB debugging on your phone, connect it and check that it is available:

```sh
adb devices
```

## Run on a phone

Napstrfy keeps the screen awake while the app is open in the foreground. The power
button still locks the phone, and the normal screen timeout applies after leaving
the app.

For a phone connected over USB, use:

```sh
npm run android:dev:usb
```

This tunnels the development server through `adb`, so the phone does not need to reach the computer over Wi-Fi.

For wireless development, use:

```sh
npm run android:dev
```

## Build and install a debug APK

```sh
npm run android:build -- --debug --apk
adb install -r src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

Pair the phone from Napstr's **Napstrfy** page. Napstr must remain running to browse or fetch its audio; songs already cached on the phone can play offline.

The Napstrfy page offers two independent, one-use QR codes:

- **Full access** keeps the usual library search, Tor download requests, and verified offline audio cache.
- **Read only** lets a phone browse, play, and cache music and audiobooks already on this Napstr. Playback uses the same verified offline audio cache, seeking, and next-track prefetching as full access. Napstr rejects requests to download new songs on the host and access its transfer history. Update both apps to get this playback behaviour.

A code says what a phone may do *to begin with*. Every right can be changed afterwards from the paired-phone list on the Napstrfy page, one tick at a time:

- **Read the library** — the index: library pages, searches within it, playlists, audiobooks, album art, comments.
- **Play and keep audio** — be handed this computer's audio and cache it.
- **Control playback** — drive the player on this computer.
- **Download from the network** — reach the network *through* this computer: relay searches, the catalogue mirror behind the Discover list, and asking it to fetch a file. Nothing here is said in anyone's name.
- **Sign and publish as you** — edit and publish playlists, post comments and file reports, and lend access on. This is the signature, and it is the one right that acts as the owner.

`Sign and publish` includes downloads, because asking this computer to fetch a song was always part of what "act as you" meant, and a phone paired before the two were separated keeps working. The useful new combination is the one that goes the other way: a phone may be lent the network without being able to publish anything.

Each code expires after five minutes. Generating a replacement affects only that code's access mode. Each paired phone shows what it may do; pairing the same phone again changes its access. Removing a phone rejects subsequent requests and stops active audio transfers at the next chunk check. Audio already cached on the phone remains available offline. Read-only access protects the host from download requests; it does not restrict copying audio.

## The phone's own key

Napstrfy generates a Nostr key for itself when it is first run and keeps it on the device. It is the phone's identity rather than the computer's: the phone's own playlists and its liked songs are filed under that key, so two phones paired with one computer have two separate sets, and the computer does not own either. Settings shows the public key and offers the secret key for export, which is what lets the same lists come back on a replacement phone.

Nothing under that key can be read or written until the phone has proved the key to the computer, which it does by signing a challenge the computer made up. That is one exchange, remembered afterwards, and it needs no connection to the network — it costs a fraction of a second. If a phone is refused with "not said which key is its own yet", it has not proved one yet; reinstalling the app is not the fix, and restoring an exported key is what keeps the same lists.

A phone's playlists therefore live in the computer's store under the phone's key rather than the computer's, which is why they survive a reinstall but not a lost key.

## Several computers at once

A phone may be paired with more than one Napstr. One of them is its **home computer**: the one whose player, download queue and status the app is drawn from, and the one it signs through. The others are libraries to read and play from, and their music appears in the same list.

The home computer is chosen in Napstrfy's Settings, which is what makes it useful to own two: a desktop and a laptop that both let the phone act as their owner are both valid homes, and only the person holding the phone knows which one is home. Without a choice it is the first computer paired that lets the phone act as its owner.

When one of them is unreachable the rest still work. The status line says how many answered (`1/2 Online`), the library and search read from whichever computers are there, and the settings list marks each one with whether it may download for this phone.

A tunnel that hears *nothing at all* is given up on after fifteen seconds and opened again, so a computer that has been asleep costs one question rather than a minute of them. That decision belongs to the transport and not to the app: QUIC heartbeats are answered by the other machine's transport rather than by its application, so silence there means the path is gone, while a computer that is slow to answer is still answering. The request level cannot tell those two apart — a request that went unanswered says nothing about the connection it was asked over — so nothing at that level drops a tunnel. The phone simply stops handing out one the transport has already closed.

Podcasts are independent of Napstr: Napstrfy searches a public podcast directory and streams or downloads episodes directly from their publishers.

The same codebase can later be built for iOS from a Mac using `npm run ios:init` and `npm run ios:dev`.
