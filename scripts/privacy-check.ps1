# Privacy verification for viewr (Windows PowerShell).
# Exit 0 only when the privacy invariants we can check locally all hold.
# Usage: pwsh -File scripts/privacy-check.ps1

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $root

Write-Host "== cargo deny (network crate ban + licenses) =="
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
# Real grants look like <key>com.apple.security.network.client</key> outside comments.
$entsLines = Get-Content "packaging/macos/viewr.entitlements"
foreach ($line in $entsLines) {
    $t = $line.Trim()
    if ($t.StartsWith("<!--") -or $t.StartsWith("-->") -or $t -match "^<!--" -or $t -match "-->") {
        continue
    }
    if ($t -match "<key>com\.apple\.security\.network\.(client|server)</key>") {
        Write-Error "macOS entitlements must not grant network client/server"
        exit 1
    }
}
if (-not (Test-Path "packaging/windows/APPCONTAINER.md")) {
    Write-Error "Missing Windows AppContainer plan"
    exit 1
}

Write-Host "== dependency tree must not pull reqwest/hyper/rustls =="
foreach ($crate in @("reqwest", "hyper", "rustls", "native-tls")) {
    $out = cargo tree -p viewr -i $crate 2>&1 | Out-String
    if ($out -notmatch "did not match any packages" -and $out -notmatch "package ID specification") {
        # cargo tree succeeds only if the package is present
        if ($LASTEXITCODE -eq 0 -and $out -match [regex]::Escape($crate)) {
            Write-Error "Forbidden network-related crate in tree: $crate"
            exit 1
        }
    }
}

Write-Host "== source must not write activity side-files or always-on logging =="
# No OpenOptions append-to-disk activity log next to user photos.
if (Select-String -Path "crates/viewr/src/app.rs" -Pattern "OpenOptions" -Quiet) {
    Write-Error "app.rs must not use OpenOptions (activity side-files are forbidden)"
    exit 1
}
$main = Get-Content "crates/viewr/src/main.rs" -Raw
if ($main -match "default_filter_or") {
    Write-Error "main.rs must not enable env_logger by default (opt-in only via RUST_LOG/VIEWR_LOG)"
    exit 1
}
if (-not (Test-Path "crates/viewr/src/ephemeral.rs")) {
    Write-Error "missing ephemeral TempWorkspace cleaner"
    exit 1
}

Write-Host "privacy-check: OK"
exit 0
