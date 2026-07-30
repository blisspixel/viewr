[CmdletBinding()]
param(
    [string]$BinaryDirectory = "target/debug"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$installer = Join-Path $repositoryRoot "install.ps1"
$binaryDirectory = [IO.Path]::GetFullPath((Join-Path $repositoryRoot $BinaryDirectory))
$mainBinary = Join-Path $binaryDirectory "viewr.exe"
$workerBinary = Join-Path $binaryDirectory "viewr-decode.exe"
foreach ($binary in @($mainBinary, $workerBinary)) {
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
        throw "missing installer smoke binary: $binary"
    }
}

$versionOutput = & $mainBinary --version
if ($LASTEXITCODE -ne 0 -or $versionOutput -notmatch '^viewr (.+)$') {
    throw "viewr binary did not report a usable version"
}
$version = $Matches[1]
$tag = "v$version"
$target = "x86_64-pc-windows-msvc"
$prefix = "viewr-$version-$target"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) (
    "installer-smoke-" + [Guid]::NewGuid().ToString("N")
)
$sourceRoot = Join-Path $testRoot $prefix
$sourceBin = Join-Path $sourceRoot "bin"
$localAppData = Join-Path $testRoot "local-app-data"
$installDir = Join-Path $localAppData "Programs\viewr"
$archiveName = "$prefix.zip"
$archivePath = Join-Path $testRoot $archiveName
$sidecarPath = "$archivePath.sha256"

function Copy-FixtureFile([string]$Source, [string]$RelativePath) {
    $destination = Join-Path $sourceRoot $RelativePath
    [IO.Directory]::CreateDirectory((Split-Path -Parent $destination)) | Out-Null
    Copy-Item -LiteralPath $Source -Destination $destination
}

function Invoke-RestMethod {
    param(
        [switch]$UseBasicParsing,
        [hashtable]$Headers,
        [string]$Uri
    )
    $null = $UseBasicParsing, $Headers, $Uri
    return $global:ViewrInstallerSmokeRelease
}

function Invoke-WebRequest {
    param(
        [switch]$UseBasicParsing,
        [hashtable]$Headers,
        [string]$Uri,
        [string]$OutFile
    )
    $null = $UseBasicParsing, $Headers, $Uri
    $fixture = if ($OutFile.EndsWith(".sha256", [StringComparison]::Ordinal)) {
        $global:ViewrInstallerSmokeSidecar
    }
    else {
        $global:ViewrInstallerSmokeArchive
    }
    Copy-Item -LiteralPath $fixture -Destination $OutFile
}

$previousLocalAppData = $env:LOCALAPPDATA
try {
    [IO.Directory]::CreateDirectory($sourceBin) | Out-Null
    Copy-FixtureFile $mainBinary "bin\viewr.exe"
    Copy-FixtureFile $workerBinary "bin\viewr-decode.exe"
    foreach ($document in @("LICENSE", "NOTICE", "README.md", "THIRD_PARTY_LICENSES.html")) {
        Copy-FixtureFile (Join-Path $repositoryRoot $document) $document
    }

    $records = [Collections.Generic.List[object]]::new()
    foreach ($file in @(
        Get-ChildItem -LiteralPath $sourceRoot -File -Recurse | Sort-Object FullName
    )) {
        $relative = [IO.Path]::GetRelativePath($sourceRoot, $file.FullName).Replace("\", "/")
        $records.Add([ordered]@{
            mode = if ($relative.StartsWith("bin/", [StringComparison]::Ordinal)) { "0755" } else { "0644" }
            path = $relative
            sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $file.FullName).Hash.ToLowerInvariant()
            size = [int64]$file.Length
        })
    }
    $manifest = [ordered]@{
        archive_format = "zip-stored-v1"
        files = $records
        package_name = "viewr"
        rust_toolchain = "1.96.0"
        schema_version = 1
        source_date_epoch = 1700000000
        target = $target
        version = $version
    }
    [IO.File]::WriteAllText(
        (Join-Path $sourceRoot "release-manifest.json"),
        (($manifest | ConvertTo-Json -Depth 6) + "`n"),
        [Text.UTF8Encoding]::new($false)
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archiveStream = [IO.File]::Open($archivePath, [IO.FileMode]::CreateNew)
    $archive = [IO.Compression.ZipArchive]::new(
        $archiveStream,
        [IO.Compression.ZipArchiveMode]::Create
    )
    try {
        foreach ($file in @(
            Get-ChildItem -LiteralPath $sourceRoot -File -Recurse | Sort-Object FullName
        )) {
            $relative = [IO.Path]::GetRelativePath($sourceRoot, $file.FullName).Replace("\", "/")
            $entry = $archive.CreateEntry(
                "$prefix/$relative",
                [IO.Compression.CompressionLevel]::NoCompression
            )
            $input = [IO.File]::OpenRead($file.FullName)
            $output = $entry.Open()
            try {
                $input.CopyTo($output)
            }
            finally {
                $output.Dispose()
                $input.Dispose()
            }
        }
    }
    finally {
        $archive.Dispose()
        $archiveStream.Dispose()
    }

    $digest = (Get-FileHash -Algorithm SHA256 -LiteralPath $archivePath).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText(
        $sidecarPath,
        "$digest  $archiveName`n",
        [Text.UTF8Encoding]::new($false)
    )
    $downloadBase = "https://github.com/blisspixel/viewr/releases/download/$tag"
    $global:ViewrInstallerSmokeArchive = $archivePath
    $global:ViewrInstallerSmokeSidecar = $sidecarPath
    $global:ViewrInstallerSmokeRelease = [pscustomobject]@{
        tag_name = $tag
        draft = $false
        prerelease = $false
        assets = @(
            [pscustomobject]@{
                name = $archiveName
                browser_download_url = "$downloadBase/$archiveName"
            },
            [pscustomobject]@{
                name = "$archiveName.sha256"
                browser_download_url = "$downloadBase/$archiveName.sha256"
            }
        )
    }

    $env:LOCALAPPDATA = $localAppData
    & $installer -Version $version -InstallDir $installDir -NoPath -NoShortcut
    $firstMarker = Join-Path $installDir ".viewr-install.json"
    if (-not (Test-Path -LiteralPath $firstMarker -PathType Leaf)) {
        $installedNames = @(Get-ChildItem -LiteralPath $installDir -Force -Recurse |
            ForEach-Object { $_.Name }) -join ", "
        throw "first installation omitted its ownership marker; files: $installedNames"
    }
    & $installer -Version $version -InstallDir $installDir -NoPath -NoShortcut

    $installedVersion = & (Join-Path $installDir "viewr.exe") --version
    if ($LASTEXITCODE -ne 0 -or $installedVersion -cne "viewr $version") {
        throw "installed fixture did not report the selected version"
    }
    $marker = Get-Content -LiteralPath (Join-Path $installDir ".viewr-install.json") -Raw |
        ConvertFrom-Json
    if ($marker.repository -cne "blisspixel/viewr" -or
        $marker.version -cne $version -or
        $marker.target -cne $target) {
        throw "installed ownership marker does not match the release"
    }
    $leftovers = @(Get-ChildItem -LiteralPath (Split-Path -Parent $installDir) -Force |
        Where-Object { $_.Name -like "viewr.installing.*" -or $_.Name -like "viewr.backup.*" })
    if ($leftovers.Count -ne 0) {
        throw "installer left staging or backup directories behind"
    }
    Write-Host "install-windows-smoke: PASS"
}
finally {
    $env:LOCALAPPDATA = $previousLocalAppData
    $global:ViewrInstallerSmokeArchive = $null
    $global:ViewrInstallerSmokeSidecar = $null
    $global:ViewrInstallerSmokeRelease = $null
    if (Test-Path -LiteralPath $testRoot) {
        [IO.Directory]::Delete($testRoot, $true)
    }
}
