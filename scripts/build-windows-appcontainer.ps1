# Build an unsigned, schema-validated local MSIX with the network-denied profile.
[CmdletBinding()]
param(
    [string]$BinaryDirectory = "target\debug"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw "Windows AppContainer packaging requires Windows"
}

$repoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location -LiteralPath $repoRoot

function Resolve-RepositoryPath([string]$Path) {
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
}

$binaryRoot = Resolve-RepositoryPath $BinaryDirectory
$profileRoot = Resolve-RepositoryPath "target\profile-check\windows"
$layout = Join-Path $profileRoot "layout"
$output = Join-Path $profileRoot "viewr-appcontainer.msix"
$profilePrefix = $profileRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
if (-not $layout.StartsWith($profilePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to replace a staging directory outside target/profile-check/windows"
}
if (-not $output.StartsWith($profilePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to replace an output outside target/profile-check/windows"
}

foreach ($candidate in @(
    (Resolve-RepositoryPath "target"),
    (Resolve-RepositoryPath "target\profile-check"),
    $profileRoot,
    $layout,
    $output
)) {
    if (Test-Path -LiteralPath $candidate) {
        $item = Get-Item -LiteralPath $candidate -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Refusing a reparse-point AppContainer staging path: $candidate"
        }
    }
}

foreach ($binary in @("viewr.exe", "viewr-decode.exe")) {
    $candidate = Join-Path $binaryRoot $binary
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
        throw "Missing packaged binary: $candidate. Run cargo build --workspace first."
    }
}

if (Test-Path -LiteralPath $layout) {
    $nestedReparsePoint = Get-ChildItem -LiteralPath $layout -Recurse -Force |
        Where-Object {
            ($_.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0
        } |
        Select-Object -First 1
    if ($null -ne $nestedReparsePoint) {
        throw "Refusing to remove staging data containing a reparse point: $($nestedReparsePoint.FullName)"
    }
    Remove-Item -LiteralPath $layout -Recurse -Force
}
if (Test-Path -LiteralPath $output) {
    Remove-Item -LiteralPath $output -Force
}
New-Item -ItemType Directory -Path (Join-Path $layout "Assets") -Force | Out-Null
New-Item -ItemType Directory -Path (Split-Path -Parent $output) -Force | Out-Null

Copy-Item -LiteralPath (Join-Path $binaryRoot "viewr.exe") -Destination $layout
Copy-Item -LiteralPath (Join-Path $binaryRoot "viewr-decode.exe") -Destination $layout
Copy-Item -LiteralPath "packaging\windows\AppxManifest.xml" -Destination $layout

Add-Type -AssemblyName System.Drawing
$icon = [System.Drawing.Icon]::new((Resolve-Path -LiteralPath "assets\icon.ico").Path)
$sourceBitmap = $icon.ToBitmap()
try {
    foreach ($asset in @(
        @{ Name = "StoreLogo.png"; Size = 50 },
        @{ Name = "Square150x150Logo.png"; Size = 150 },
        @{ Name = "Square44x44Logo.png"; Size = 44 }
    )) {
        $bitmap = [System.Drawing.Bitmap]::new($asset.Size, $asset.Size)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        try {
            $graphics.Clear([System.Drawing.Color]::Transparent)
            $graphics.DrawImage($sourceBitmap, 0, 0, $asset.Size, $asset.Size)
            $bitmap.Save(
                (Join-Path $layout "Assets\$($asset.Name)"),
                [System.Drawing.Imaging.ImageFormat]::Png
            )
        }
        finally {
            $graphics.Dispose()
            $bitmap.Dispose()
        }
    }
}
finally {
    $sourceBitmap.Dispose()
    $icon.Dispose()
}

$kitsBin = "C:\Program Files (x86)\Windows Kits\10\bin"
$makeAppx = Get-ChildItem -LiteralPath $kitsBin -Recurse -Filter makeappx.exe |
    Where-Object { $_.Directory.Name -eq "x64" } |
    Sort-Object { [version]$_.Directory.Parent.Name } -Descending |
    Select-Object -First 1
if ($null -eq $makeAppx) {
    throw "MakeAppx.exe was not found in the Windows 10 SDK"
}

& $makeAppx.FullName pack /d $layout /p $output /o
if ($LASTEXITCODE -ne 0) {
    throw "MakeAppx.exe rejected the AppContainer package"
}
if (-not (Test-Path -LiteralPath $output -PathType Leaf)) {
    throw "MakeAppx.exe did not create $output"
}

Write-Output "AppContainer package validated: $output"
