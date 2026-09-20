Release artifacts are no longer copied into the website. The download page discovers the
versioned installers attached to the latest published GitHub Release.

Create a `v*` tag to run `.github/workflows/release.yml`; the Tauri action builds and attaches
the Windows installer, Linux AppImage, and architecture-specific macOS community DMGs automatically.
The same tag runs `.github/workflows/napstrfy-desktop.yml` for Napstrfy desktop installers;
`release.yml` also attaches the Napstrfy Android APK.

Both product pages query GitHub's latest published release on every visit. The Napstrfy
page lists Windows, Linux, macOS (Apple Silicon and Intel), and Android separately.
Only matching product assets enable download links. Missing installers display as
unavailable and become downloadable on the next visit after they are attached to the
latest release, without rebuilding or redeploying the website. Draft releases and CI
artifacts are not public downloads. If GitHub's API is unavailable, use the View release link.
