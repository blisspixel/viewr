#Requires -Version 5.1

[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\viewr",
    [switch]$NoPath,
    [switch]$NoShortcut
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repository = "blisspixel/viewr"
$apiBase = "https://api.github.com/repos/$repository"
$releaseBase = "https://github.com/$repository/releases"
$target = "x86_64-pc-windows-msvc"

function Stop-Install([string]$Message) {
    throw "viewr installer: $Message"
}

function Get-Sha256([string]$Path) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

if (-not [Environment]::Is64BitOperatingSystem) {
    Stop-Install "a 64-bit Windows installation is required"
}
if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
    Stop-Install "LOCALAPPDATA is not available"
}

$InstallDir = [IO.Path]::GetFullPath($InstallDir)
$localPrograms = [IO.Path]::GetFullPath("$env:LOCALAPPDATA\Programs")
if (-not $InstallDir.StartsWith($localPrograms + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    Stop-Install "InstallDir must be inside the current user's LocalAppData Programs directory"
}

[Net.ServicePointManager]::SecurityProtocol =
    [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$headers = @{
    Accept = "application/vnd.github+json"
    "User-Agent" = "viewr-installer"
    "X-GitHub-Api-Version" = "2022-11-28"
}

try {
    if ($Version -eq "latest") {
        $requestedTag = $null
        $release = Invoke-RestMethod -UseBasicParsing -Headers $headers -Uri "$apiBase/releases/latest"
    }
    else {
        $requestedTag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
        $release = Invoke-RestMethod -UseBasicParsing -Headers $headers -Uri "$apiBase/releases/tags/$requestedTag"
    }
}
catch {
    Stop-Install "could not resolve the selected official GitHub release"
}

$tag = [string]$release.tag_name
if ($tag -notmatch '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)([-+][0-9A-Za-z.-]+)?$') {
    Stop-Install "GitHub returned an invalid release tag"
}
if ([bool]$release.draft -or [bool]$release.prerelease) {
    Stop-Install "the selected release is not a published stable release"
}
if ($null -ne $requestedTag -and $tag -cne $requestedTag) {
    Stop-Install "GitHub returned a release other than the requested version"
}
$releaseVersion = $tag.Substring(1)
$archiveName = "viewr-$releaseVersion-$target.zip"
$checksumName = "$archiveName.sha256"
$expectedDownloadPrefix = "$releaseBase/download/$tag/"
$archiveAsset = @($release.assets | Where-Object { $_.name -ceq $archiveName })
$checksumAsset = @($release.assets | Where-Object { $_.name -ceq $checksumName })
if ($archiveAsset.Count -ne 1 -or $checksumAsset.Count -ne 1) {
    Stop-Install "the release does not contain exactly one Windows archive and checksum"
}
foreach ($asset in @($archiveAsset[0], $checksumAsset[0])) {
    $expectedAssetUrl = "$expectedDownloadPrefix$([string]$asset.name)"
    if ([string]$asset.browser_download_url -cne $expectedAssetUrl) {
        Stop-Install "GitHub returned an unexpected release asset URL"
    }
}

$temporary = Join-Path ([IO.Path]::GetTempPath()) ("viewr-install-" + [Guid]::NewGuid().ToString("N"))
[IO.Directory]::CreateDirectory($temporary) | Out-Null
try {
    $archivePath = Join-Path $temporary $archiveName
    $checksumPath = Join-Path $temporary $checksumName
    Write-Host "Downloading viewr $releaseVersion for Windows x64..."
    Invoke-WebRequest -UseBasicParsing -Headers $headers -Uri $archiveAsset[0].browser_download_url -OutFile $archivePath
    Invoke-WebRequest -UseBasicParsing -Headers $headers -Uri $checksumAsset[0].browser_download_url -OutFile $checksumPath

    $checksumText = [IO.File]::ReadAllText($checksumPath).Replace("`r", "")
    $checksumMatch = [regex]::Match(
        $checksumText,
        "\A([0-9a-f]{64})  $([regex]::Escape($archiveName))`n\z",
        [Text.RegularExpressions.RegexOptions]::CultureInvariant
    )
    if (-not $checksumMatch.Success) {
        Stop-Install "release checksum has an invalid format"
    }
    if ((Get-Sha256 $archivePath) -cne $checksumMatch.Groups[1].Value) {
        Stop-Install "release archive checksum mismatch"
    }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $prefix = "viewr-$releaseVersion-$target"
    $archive = [IO.Compression.ZipFile]::OpenRead($archivePath)
    try {
        $entries = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        foreach ($entry in $archive.Entries) {
            $name = $entry.FullName
            if (-not $entries.Add($name)) {
                Stop-Install "release archive contains a duplicate path"
            }
            $relativeArchivePath = if ($name.StartsWith("$prefix/", [StringComparison]::Ordinal)) {
                $name.Substring($prefix.Length + 1)
            }
            else {
                ""
            }
            $archiveSegments = @($relativeArchivePath.Split('/'))
            if ([string]::IsNullOrWhiteSpace($entry.Name) -or
                [string]::IsNullOrWhiteSpace($relativeArchivePath) -or
                $name.Contains("\") -or $name.Contains(":") -or
                @($archiveSegments | Where-Object { $_ -in @("", ".", "..") }).Count -ne 0) {
                Stop-Install "release archive contains an unsafe path"
            }
        }
    }
    finally {
        $archive.Dispose()
    }

    $extractRoot = Join-Path $temporary "extract"
    [IO.Compression.ZipFile]::ExtractToDirectory($archivePath, $extractRoot)
    $sourceRoot = Join-Path $extractRoot $prefix
    $manifestPath = Join-Path $sourceRoot "release-manifest.json"
    foreach ($required in @(
        "bin\viewr.exe",
        "bin\viewr-decode.exe",
        "LICENSE",
        "NOTICE",
        "README.md",
        "THIRD_PARTY_LICENSES.html",
        "release-manifest.json"
    )) {
        $requiredPath = Join-Path $sourceRoot $required
        if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf) -or
            ((Get-Item -LiteralPath $requiredPath).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            Stop-Install "release archive is missing a required regular file: $required"
        }
    }

    try {
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    }
    catch {
        Stop-Install "release manifest is not valid JSON"
    }
    if ($manifest.package_name -cne "viewr" -or
        $manifest.version -cne $releaseVersion -or
        $manifest.target -cne $target) {
        Stop-Install "release manifest identity does not match the selected release"
    }
    $manifestPaths = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $expectedArchivePaths = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $expectedArchivePaths.Add("$prefix/release-manifest.json") | Out-Null
    foreach ($record in @($manifest.files)) {
        $relative = [string]$record.path
        $segments = @($relative.Split('/'))
        $recordProperties = @($record.PSObject.Properties.Name)
        $expectedMode = if ($relative.StartsWith("bin/", [StringComparison]::Ordinal)) { "0755" } else { "0644" }
        if ([string]::IsNullOrWhiteSpace($relative) -or
            $relative.Contains("\") -or $relative.Contains(":") -or
            @($segments | Where-Object { $_ -in @("", ".", "..") }).Count -ne 0 -or
            $recordProperties.Count -ne 4 -or
            @($recordProperties | Where-Object { $_ -notin @("mode", "path", "sha256", "size") }).Count -ne 0 -or
            [string]$record.mode -cne $expectedMode -or
            [string]$record.sha256 -cnotmatch '^[0-9a-f]{64}$' -or
            ($record.size -isnot [int] -and $record.size -isnot [long]) -or
            [int64]$record.size -lt 0 -or
            -not $manifestPaths.Add($relative)) {
            Stop-Install "release manifest contains an unsafe or duplicate path"
        }
        $expectedArchivePaths.Add("$prefix/$relative") | Out-Null
        $payload = Join-Path $sourceRoot ($relative.Replace('/', [IO.Path]::DirectorySeparatorChar))
        if (-not (Test-Path -LiteralPath $payload -PathType Leaf)) {
            Stop-Install "release manifest names a missing file"
        }
        $payloadItem = Get-Item -LiteralPath $payload
        if (($payloadItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
            $payloadItem.Length -ne [int64]$record.size -or
            (Get-Sha256 $payload) -cne [string]$record.sha256) {
            Stop-Install "release manifest verification failed for $relative"
        }
    }
    if ($manifestPaths.Count -eq 0 -or -not $entries.SetEquals($expectedArchivePaths)) {
        Stop-Install "release archive file set does not match its manifest"
    }

    $installParent = Split-Path -Parent $InstallDir
    [IO.Directory]::CreateDirectory($installParent) | Out-Null
    if (Test-Path -LiteralPath $InstallDir) {
        $existing = Get-Item -LiteralPath $InstallDir
        if (-not $existing.PSIsContainer -or ($existing.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            Stop-Install "existing install path is not a regular directory"
        }
        $marker = Join-Path $InstallDir ".viewr-install.json"
        if (-not (Test-Path -LiteralPath $marker -PathType Leaf)) {
            $legacyNames = @(Get-ChildItem -LiteralPath $InstallDir -Force | ForEach-Object { $_.Name })
            $unexpected = @($legacyNames | Where-Object { $_ -notin @("viewr.exe", "viewr-decode.exe") })
            if ($unexpected.Count -ne 0 -or
                -not (Test-Path -LiteralPath (Join-Path $InstallDir "viewr.exe") -PathType Leaf) -or
                -not (Test-Path -LiteralPath (Join-Path $InstallDir "viewr-decode.exe") -PathType Leaf)) {
                Stop-Install "refusing to replace an installation not owned by the viewr installer"
            }
            foreach ($legacyName in @("viewr.exe", "viewr-decode.exe")) {
                if ((Get-Item -LiteralPath (Join-Path $InstallDir $legacyName)).Attributes -band
                    [IO.FileAttributes]::ReparsePoint) {
                    Stop-Install "legacy installation contains a linked executable"
                }
            }
        }
        else {
            $markerItem = Get-Item -LiteralPath $marker
            if ($markerItem.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                Stop-Install "existing installer ownership marker is not a regular file"
            }
            try {
                $ownership = Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json
            }
            catch {
                Stop-Install "existing installer ownership marker is invalid"
            }
            if ($ownership.repository -cne $repository) {
                Stop-Install "refusing to replace an installation with a foreign ownership marker"
            }
            $allowedNames = @(
                ".viewr-install.json",
                "viewr.exe",
                "viewr-decode.exe",
                "LICENSE",
                "NOTICE",
                "README.md",
                "THIRD_PARTY_LICENSES.html",
                "release-manifest.json"
            )
            foreach ($child in @(Get-ChildItem -LiteralPath $InstallDir -Force)) {
                if ($child.PSIsContainer -or
                    ($child.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
                    $child.Name -notin $allowedNames) {
                    Stop-Install "installer-owned directory contains an unexpected path: $($child.Name)"
                }
            }
        }
    }

    $stage = Join-Path $installParent ("viewr.installing." + [Guid]::NewGuid().ToString("N"))
    $backup = Join-Path $installParent ("viewr.backup." + [Guid]::NewGuid().ToString("N"))
    [IO.Directory]::CreateDirectory($stage) | Out-Null
    try {
        Copy-Item -LiteralPath (Join-Path $sourceRoot "bin\viewr.exe") -Destination $stage
        Copy-Item -LiteralPath (Join-Path $sourceRoot "bin\viewr-decode.exe") -Destination $stage
        foreach ($document in @("LICENSE", "NOTICE", "THIRD_PARTY_LICENSES.html", "README.md", "release-manifest.json")) {
            Copy-Item -LiteralPath (Join-Path $sourceRoot $document) -Destination $stage
        }
        $markerObject = [ordered]@{
            repository = $repository
            version = $releaseVersion
            target = $target
        }
        [IO.File]::WriteAllText(
            (Join-Path $stage ".viewr-install.json"),
            (($markerObject | ConvertTo-Json -Compress) + "`n"),
            [Text.UTF8Encoding]::new($false)
        )

        $stagedVersion = & (Join-Path $stage "viewr.exe") --version
        if ($LASTEXITCODE -ne 0 -or $stagedVersion -cne "viewr $releaseVersion") {
            Stop-Install "staged binary version does not match the selected release"
        }
        & (Join-Path $stage "viewr.exe") doctor | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Stop-Install "staged binaries did not pass viewr doctor"
        }

        $hadPrevious = Test-Path -LiteralPath $InstallDir
        if ($hadPrevious) {
            Move-Item -LiteralPath $InstallDir -Destination $backup
        }
        try {
            Move-Item -LiteralPath $stage -Destination $InstallDir
        }
        catch {
            if ($hadPrevious -and (Test-Path -LiteralPath $backup) -and -not (Test-Path -LiteralPath $InstallDir)) {
                Move-Item -LiteralPath $backup -Destination $InstallDir
            }
            throw
        }
        if ($hadPrevious -and (Test-Path -LiteralPath $backup)) {
            try {
                [IO.Directory]::Delete($backup, $true)
            }
            catch {
                Write-Warning "The previous installer-owned directory remains at $backup and can be removed after inspection."
            }
        }
    }
    catch {
        if (Test-Path -LiteralPath $stage) {
            [IO.Directory]::Delete($stage, $true)
        }
        throw
    }

    if (-not $NoPath) {
        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        $segments = @($userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
        if (-not ($segments | Where-Object { $_.TrimEnd('\') -ieq $InstallDir.TrimEnd('\') })) {
            $updatedPath = (($segments + $InstallDir) -join ';')
            [Environment]::SetEnvironmentVariable("Path", $updatedPath, "User")
        }
        if (-not (($env:Path -split ';') | Where-Object { $_.TrimEnd('\') -ieq $InstallDir.TrimEnd('\') })) {
            $env:Path = "$InstallDir;$env:Path"
        }
    }

    if (-not $NoShortcut) {
        try {
            $programs = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
            $shortcutPath = Join-Path $programs "viewr.lnk"
            $shell = New-Object -ComObject WScript.Shell
            $shortcut = $shell.CreateShortcut($shortcutPath)
            $shortcut.TargetPath = Join-Path $InstallDir "viewr.exe"
            $shortcut.WorkingDirectory = $InstallDir
            $shortcut.IconLocation = Join-Path $InstallDir "viewr.exe"
            $shortcut.Save()
        }
        catch {
            Write-Warning "viewr was installed, but the Start menu shortcut could not be created."
        }
    }

    $installedVersion = & (Join-Path $InstallDir "viewr.exe") --version
    if ($LASTEXITCODE -ne 0 -or $installedVersion -cne "viewr $releaseVersion") {
        Stop-Install "installed binary version does not match the selected release"
    }
    Write-Host "Installed $installedVersion in $InstallDir"
    Write-Host "Run viewr from PowerShell or the Start menu."
    Write-Host "Updates are explicit: run this installer again. viewr performs no background checks."
}
finally {
    if (Test-Path -LiteralPath $temporary) {
        [IO.Directory]::Delete($temporary, $true)
    }
}
