# Privacy verification for viewr (Windows PowerShell).
# Exit 0 only when the privacy invariants we can check locally all hold.
# Usage: pwsh -File scripts/privacy-check.ps1

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $root

Write-Host "== cargo deny (remote-client ban + confined Linux D-Bus + licenses) =="
cargo deny check
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== packaging artifacts must omit network grants =="
# Only flag real finish-args, not comments that say "do NOT add --share=network".
$flatpakLines = Get-Content "packaging/flatpak/com.github.blisspixel.viewr.yml"
foreach ($line in $flatpakLines) {
    $t = $line.Trim()
    if ($t.StartsWith("#")) { continue }
    if ($t -match "--share=network") {
        Write-Error "Flatpak manifest must not grant --share=network"
        exit 1
    }
}
foreach ($entitlementsPath in @(
    "packaging/macos/viewr.entitlements",
    "packaging/macos/viewr-decode.entitlements"
)) {
    [xml]$entitlements = Get-Content -LiteralPath $entitlementsPath -Raw
    $keys = $entitlements.SelectNodes("/*[local-name()='plist']/*[local-name()='dict']/*[local-name()='key']") |
        ForEach-Object { $_.InnerText }
    if ($keys -match "^com\.apple\.security\.network\.(client|server)$") {
        Write-Error "$entitlementsPath must not grant network client/server"
        exit 1
    }
}

$appxPath = "packaging/windows/AppxManifest.xml"
if (-not (Test-Path -LiteralPath $appxPath -PathType Leaf)) {
    Write-Error "Missing Windows AppContainer manifest"
    exit 1
}
[xml]$appx = Get-Content -LiteralPath $appxPath -Raw
$namespaces = [System.Xml.XmlNamespaceManager]::new($appx.NameTable)
$namespaces.AddNamespace("f", "http://schemas.microsoft.com/appx/manifest/foundation/windows10")
$namespaces.AddNamespace("uap10", "http://schemas.microsoft.com/appx/manifest/uap/windows10/10")
$application = $appx.SelectSingleNode("/f:Package/f:Applications/f:Application", $namespaces)
if ($null -eq $application -or
    $application.GetAttribute("TrustLevel", $namespaces.LookupNamespace("uap10")) -ne "appContainer" -or
    $application.GetAttribute("RuntimeBehavior", $namespaces.LookupNamespace("uap10")) -ne "packagedClassicApp") {
    Write-Error "Windows package must run as a packagedClassicApp AppContainer"
    exit 1
}
$capabilities = $appx.SelectSingleNode("/f:Package/f:Capabilities", $namespaces)
if ($null -eq $capabilities -or $capabilities.SelectNodes("*").Count -ne 0) {
    Write-Error "Windows AppContainer must have an explicit empty capability set"
    exit 1
}

Write-Host "== dependency tree must not pull remote-service client stacks =="
$tree = cargo tree --quiet -p viewr --prefix none --edges normal | Out-String
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
foreach ($crate in @("reqwest", "hyper", "rustls", "native-tls")) {
    $packageLine = "(?m)^$([regex]::Escape($crate)) v"
    if ($tree -match $packageLine) {
        Write-Error "Forbidden network-related crate in tree: $crate"
        exit 1
    }
}

Write-Host "== narrow source privacy tripwires + ephemeral contracts =="
# This orchestration check is a regression tripwire, not a complete Rust
# write-path analyzer. Default logger behavior is covered by Rust tests.
if (Select-String -Path "crates/viewr/src/app.rs" -Pattern "OpenOptions" -Quiet) {
    Write-Error "app.rs must not acquire direct OpenOptions persistence capability"
    exit 1
}
if (-not (Test-Path "crates/viewr/src/ephemeral.rs")) {
    Write-Error "missing ephemeral TempWorkspace cleaner"
    exit 1
}
$eph = Get-Content "crates/viewr/src/ephemeral.rs" -Raw
if ($eph -notmatch [regex]::Escape('std::fs::create_dir(&path)?')) {
    Write-Error "TempWorkspace must atomically create its exact path"
    exit 1
}
if ($eph -match "scrub_stale_viewr_temps|read_dir\s*\(\s*&?root") {
    Write-Error "ephemeral.rs must not sweep the shared system temp root"
    exit 1
}
$cli = Get-Content "crates/viewr/src/cli.rs" -Raw
if ($cli -notmatch "load_from_memory") {
    Write-Error "cli doctor/benchmark must use in-memory decode (load_from_memory)"
    exit 1
}
$mainSrc = Get-Content "crates/viewr/src/main.rs" -Raw
if ($mainSrc -match "scrub_stale_viewr_temps") {
    Write-Error "main.rs must not perform shared temp-root cleanup on launch"
    exit 1
}

Write-Host "privacy-check: OK"
exit 0
