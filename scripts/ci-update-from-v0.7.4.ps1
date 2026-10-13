# Install published v0.7.4 with its own installer, then update to the candidate
# archive already packaged in target/release-artifacts. No release is published.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $repo

$candidateDir = Join-Path $repo "target\release-artifacts"
$candidateArchive = Join-Path $candidateDir "viewr-0.7.4-x86_64-pc-windows-msvc.zip"
$candidateBinary = Join-Path $repo "target\release\viewr.exe"
if (-not (Test-Path -LiteralPath $candidateArchive -PathType Leaf)) {
    throw "missing candidate archive: $candidateArchive"
}
if (-not (Test-Path -LiteralPath $candidateBinary -PathType Leaf)) {
    throw "missing candidate binary: $candidateBinary"
}
python -B scripts/release_artifact.py verify $candidateArchive
if ($LASTEXITCODE -ne 0) {
    throw "candidate archive verification failed"
}

$published = Join-Path $env:RUNNER_TEMP "viewr-v074-published"
New-Item -ItemType Directory -Force -Path $published | Out-Null
gh release download v0.7.4 --repo blisspixel/viewr --clobber `
    --pattern "viewr-0.7.4-x86_64-pc-windows-msvc.zip" `
    --pattern "viewr-0.7.4-x86_64-pc-windows-msvc.zip.sha256" `
    --pattern "install.ps1" `
    --dir $published
if ($LASTEXITCODE -ne 0) {
    throw "could not download published v0.7.4"
}

$publishedArchive = Join-Path $published "viewr-0.7.4-x86_64-pc-windows-msvc.zip"
$publishedInstaller = Join-Path $published "install.ps1"
gh release verify v0.7.4 --repo blisspixel/viewr
if ($LASTEXITCODE -ne 0) { throw "gh release verify failed" }
gh release verify-asset v0.7.4 $publishedArchive --repo blisspixel/viewr
if ($LASTEXITCODE -ne 0) { throw "published archive attestation failed" }
gh release verify-asset v0.7.4 $publishedInstaller --repo blisspixel/viewr
if ($LASTEXITCODE -ne 0) { throw "published installer attestation failed" }
gh attestation verify $publishedArchive `
    --repo blisspixel/viewr `
    --signer-workflow blisspixel/viewr/.github/workflows/release.yml `
    --source-ref refs/tags/v0.7.4 `
    --deny-self-hosted-runners
if ($LASTEXITCODE -ne 0) { throw "published archive provenance failed" }
gh attestation verify $publishedInstaller `
    --repo blisspixel/viewr `
    --signer-workflow blisspixel/viewr/.github/workflows/release.yml `
    --source-ref refs/tags/v0.7.4 `
    --deny-self-hosted-runners
if ($LASTEXITCODE -ne 0) { throw "published installer provenance failed" }
python -B scripts/release_artifact.py verify $publishedArchive
if ($LASTEXITCODE -ne 0) { throw "published archive contract failed" }

$root = Join-Path $env:RUNNER_TEMP "viewr-update-home"
$env:LOCALAPPDATA = Join-Path $root "local"
$env:APPDATA = Join-Path $root "roaming"
New-Item -ItemType Directory -Force -Path (Join-Path $env:LOCALAPPDATA "Programs") | Out-Null
$preferenceDir = Join-Path $env:APPDATA "viewr"
New-Item -ItemType Directory -Force -Path $preferenceDir | Out-Null
$installDir = Join-Path $env:LOCALAPPDATA "Programs\viewr"

$utf8 = [Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText((Join-Path $preferenceDir "folder-sort"), "name`n", $utf8)
[IO.File]::WriteAllText((Join-Path $preferenceDir "appearance"), "dark`n", $utf8)
[IO.File]::WriteAllText((Join-Path $preferenceDir "language"), "en`n", $utf8)

$classesBefore = Join-Path $root "classes-before.reg"
$classesAfter = Join-Path $root "classes-after.reg"
& reg.exe export "HKCU\Software\Classes" $classesBefore /y
if ($LASTEXITCODE -ne 0) { throw "could not export HKCU\Software\Classes" }

& $publishedInstaller -Version 0.7.4 -InstallDir $installDir -NoPath -NoShortcut
& "$PSScriptRoot\..\install.ps1" -Version 0.7.4 -InstallDir $installDir -NoPath -NoShortcut -LocalRelease $candidateDir

$installed = Join-Path $installDir "viewr.exe"
$reported = (& $installed --version).Trim()
if ($reported -cne "viewr 0.7.4") {
    throw "installed version was '$reported'"
}
$installedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installed).Hash
$candidateHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $candidateBinary).Hash
if ($installedHash -cne $candidateHash) {
    throw "installed binary does not match the candidate build"
}
foreach ($name in @("folder-sort", "appearance", "language")) {
    $expected = switch ($name) {
        "folder-sort" { "name`n" }
        "appearance" { "dark`n" }
        "language" { "en`n" }
    }
    $actual = [IO.File]::ReadAllText((Join-Path $preferenceDir $name))
    if ($actual -cne $expected) {
        throw "preference changed: $name"
    }
}
$parent = Split-Path -Parent $installDir
$leftovers = @(Get-ChildItem -LiteralPath $parent -Force | Where-Object {
    $_.Name -like "viewr.installing.*" -or $_.Name -like "viewr.backup.*"
})
if ($leftovers.Count -ne 0) {
    throw "installer staging remains: $($leftovers.Name -join ', ')"
}

& reg.exe export "HKCU\Software\Classes" $classesAfter /y
if ($LASTEXITCODE -ne 0) { throw "could not re-export HKCU\Software\Classes" }
$beforeHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $classesBefore).Hash
$afterHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $classesAfter).Hash
if ($beforeHash -cne $afterHash) {
    throw "HKCU\Software\Classes changed"
}

Write-Host "updated published v0.7.4 to the candidate; HKCU\Software\Classes unchanged"
