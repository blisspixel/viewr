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
$flatpak = Get-Content "packaging/flatpak/com.github.viewr.viewr.yml" -Raw
if ($flatpak -match "--share=network") {
    Write-Error "Flatpak manifest must not contain --share=network"
    exit 1
}
$ents = Get-Content "packaging/macos/viewr.entitlements" -Raw
if ($ents -match "network\.client" -or $ents -match "network\.server") {
    Write-Error "macOS entitlements must not grant network client/server"
    exit 1
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

Write-Host "privacy-check: OK"
exit 0
