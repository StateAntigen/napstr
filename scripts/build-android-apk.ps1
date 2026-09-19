#Requires -Version 5.1
<#
.SYNOPSIS
    Builds, signs and verifies an installable Napstrfy APK.

.DESCRIPTION
    Four things have to line up on this machine, and three of them fail in ways
    that do not look like their cause:

      * The Tauri CLI needs JAVA_HOME, ANDROID_HOME and NDK_HOME. Without
        NDK_HOME it tries to install one, cannot do that non-interactively (it
        looks for sdkmanager under cmdline-tools\bin, not cmdline-tools\latest),
        and fails somewhere unrelated.
      * A release APK comes out genuinely unsigned - no META-INF/*.RSA - so it
        cannot be installed as the build leaves it.
      * apksigner MUST run under JDK 17. The `java` on PATH is Java 8, which
        cannot read the PKCS12 debug keystore and reports "Invalid keystore
        format" or fails in PBES2Parameters. Both are the same Java 8 limitation
        and neither means the keystore is damaged.
      * Windows Developer Mode must be on, or the build dies at the jniLibs step
        with "Creation symbolic link is not allowed for this system".

    Signing uses the Android debug key, which is the key the installed app was
    signed with. That is what lets a new APK upgrade it in place. A different key
    cannot upgrade an installed app - it means uninstalling and pairing again.

.PARAMETER Debug
    Build the debug variant instead of release. Keeps symbols, so it is much
    larger (~91 MB against ~36 MB).

.PARAMETER NoSign
    Stop once the build finishes, leaving the unsigned APK where it landed.

.PARAMETER Install
    Install the finished APK over adb. Fails loudly when no device is attached.

.PARAMETER PreflightOnly
    Check the toolchain and free space, then stop. No build, no signing.

.EXAMPLE
    pwsh -File scripts/build-android-apk.ps1
    Builds the release APK, aligns and signs it, verifies the signature, and
    writes the installable to the Desktop.

.EXAMPLE
    pwsh -File scripts/build-android-apk.ps1 -PreflightOnly
    Verifies everything the build needs without starting one.
#>
# Deliberately not [CmdletBinding()]: that would add PowerShell's own common
# -Debug switch, which collides with the one below. Here -Debug means the debug
# APK, and nothing in this script wants the common parameters.
param(
    [switch]$Debug,
    [switch]$NoSign,
    [switch]$Install,
    [switch]$PreflightOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Absolute paths throughout, derived from this script's own location. A mangled
# lead-in in a one-liner once turned every path into C:\android\..., so nothing
# here depends on the current working directory.
$root       = Split-Path -Parent $PSScriptRoot
$android    = Join-Path $root 'android'
$project    = Join-Path $android 'src-tauri\gen\android'
$outputs    = Join-Path $project 'app\build\outputs\apk'
$configFile = Join-Path $android 'src-tauri\tauri.conf.json'

$local      = $env:LOCALAPPDATA
$node       = Join-Path $local 'Programs\nodejs\node-v24.19.0-win-x64'
$jdk        = Join-Path $local 'Programs\jdk-17.0.20.1+1'
$sdk        = Join-Path $local 'Android\Sdk'
$ndk        = Join-Path $sdk 'ndk\29.0.13846066'
$buildTools = Join-Path $sdk 'build-tools\36.0.0'
$zipalign   = Join-Path $buildTools 'zipalign.exe'
$apksigner  = Join-Path $buildTools 'lib\apksigner.jar'
$keystore   = Join-Path $env:USERPROFILE '.android\debug.keystore'
$adb        = Join-Path $sdk 'platform-tools\adb.exe'

function Write-Step($message) {
    Write-Host ''
    Write-Host "== $message" -ForegroundColor Cyan
}

Write-Step 'Checking the toolchain'

$required = [ordered]@{
    'Node v24'          = (Join-Path $node 'node.exe')
    'JDK 17'            = (Join-Path $jdk 'bin\java.exe')
    'Android SDK'       = $sdk
    'NDK 29.0.13846066' = $ndk
    'zipalign'          = $zipalign
    'apksigner'         = $apksigner
    'debug keystore'    = $keystore
    'generated project' = $project
}
$missing = @($required.GetEnumerator() | Where-Object { -not (Test-Path -LiteralPath $_.Value) })
if ($missing.Count -gt 0) {
    foreach ($item in $missing) {
        Write-Host ("  missing: {0}`n           {1}" -f $item.Key, $item.Value) -ForegroundColor Red
    }
    throw 'The Android toolchain is incomplete, so the build would fail partway through.'
}
foreach ($item in $required.GetEnumerator()) { Write-Host "  ok  $($item.Key)" }

# The build symlinks its own .so into jniLibs, which Windows only allows with
# Developer Mode on. There is no way to make the CLI copy instead, and Gradle
# delegates back to the CLI, so running gradlew directly does not avoid it.
$devMode = 0
try {
    $devMode = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock' -ErrorAction Stop).AllowDevelopmentWithoutDevLicense
}
catch {
    $devMode = 0
}
if ($devMode -ne 1) {
    throw 'Windows Developer Mode is off. The build needs it to symlink the libraries. Turn it on under Settings > System > For developers.'
}
Write-Host '  ok  Developer Mode'

# Fail here rather than fifteen minutes into a Rust build that runs out of room.
# A cold Android build reaches several GB; a warm one needs far less.
$freeGb = (Get-PSDrive C).Free / 1GB
Write-Host ('  free space: {0:N2} GB' -f $freeGb)
if ($freeGb -lt 3) {
    throw ('Only {0:N2} GB free. A cold Android Rust build needs several GB, so free space before starting.' -f $freeGb)
}

if ($PreflightOnly) {
    Write-Step 'Preflight only'
    Write-Host '  Everything the build needs is present.' -ForegroundColor Green
    return
}

$env:PATH = "$node;$env:PATH"
$env:JAVA_HOME = $jdk
$env:ANDROID_HOME = $sdk
$env:NDK_HOME = $ndk
# Keeps the debug library slim - 368 MB becomes 82 MB. Only affects the dev
# profile, so it is harmless for a release build.
$env:CARGO_PROFILE_DEV_DEBUG = '0'

$variant = if ($Debug) { 'debug' } else { 'release' }

Write-Step "Building the $variant APK"

# A stale APK in the output directory is how a failed or skipped build looks like
# a successful one, so the directory goes first.
if (Test-Path -LiteralPath $outputs) {
    Remove-Item -LiteralPath $outputs -Recurse -Force
}

$buildArgs = @('--apk', '--target', 'aarch64')
if ($Debug) { $buildArgs = @('--debug') + $buildArgs }

Push-Location $android
try {
    & npm run android:build -- @buildArgs
    if ($LASTEXITCODE -ne 0) { throw "The APK build failed with exit code $LASTEXITCODE." }
}
finally {
    Pop-Location
}

$built = @(Get-ChildItem -LiteralPath $outputs -Recurse -Filter '*.apk' -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending)
if ($built.Count -eq 0) { throw "The build finished but produced no APK under $outputs" }
$built = $built[0]
Write-Host ("  built {0}  ({1:N1} MB)" -f $built.Name, ($built.Length / 1MB))

if ($NoSign) {
    Write-Step 'Not signing, as asked'
    Write-Host "  unsigned APK: $($built.FullName)"
    Write-Host '  note: an unsigned APK cannot be installed.' -ForegroundColor Yellow
    return
}

Write-Step 'Aligning and signing'

$version = '0.1.0'
if (Test-Path -LiteralPath $configFile) {
    try {
        $parsed = (Get-Content -LiteralPath $configFile -Raw | ConvertFrom-Json).version
        if ($parsed) { $version = $parsed }
    }
    catch {
        # A version this cannot read is not a reason to refuse to name the file.
    }
}
$final = Join-Path ([Environment]::GetFolderPath('Desktop')) "Napstrfy-$version-arm64.apk"
$aligned = Join-Path $env:TEMP 'napstrfy-aligned.apk'

# zipalign must run before signing, and the order is not interchangeable.
& $zipalign -p -f 4 $built.FullName $aligned
if ($LASTEXITCODE -ne 0) { throw 'zipalign failed.' }

# JDK 17's java explicitly: the one on PATH cannot read a PKCS12 keystore.
# --ks-type PKCS12 is required, because apksigner otherwise assumes JKS.
& (Join-Path $jdk 'bin\java.exe') -jar $apksigner sign `
    --ks $keystore `
    --ks-type PKCS12 `
    --ks-pass pass:android `
    --key-pass pass:android `
    --ks-key-alias androiddebugkey `
    --out $final `
    $aligned
if ($LASTEXITCODE -ne 0) { throw 'apksigner failed to sign the APK.' }

Remove-Item -LiteralPath $aligned -Force -ErrorAction SilentlyContinue

Write-Step 'Verifying the signature'
& (Join-Path $jdk 'bin\java.exe') -jar $apksigner verify --print-certs $final
if ($LASTEXITCODE -ne 0) { throw 'The signed APK did not verify.' }

Write-Step 'Done'
Write-Host ("  {0}" -f $final)
Write-Host ("  {0:N2} MB" -f ((Get-Item -LiteralPath $final).Length / 1MB))
Write-Host '  The certificate digest above must match the installed app for this to'
Write-Host '  upgrade in place. A different digest means uninstall, then pair again.'

if ($Install) {
    Write-Step 'Installing over adb'
    if (-not (Test-Path -LiteralPath $adb)) { throw "adb is missing at $adb" }
    $attached = @(& $adb devices | Select-String '\sdevice$')
    if ($attached.Count -eq 0) { throw 'No device is attached, so there is nothing to install onto.' }
    & $adb install -r $final
    if ($LASTEXITCODE -ne 0) { throw 'adb install failed.' }
    Write-Host '  installed'
}
