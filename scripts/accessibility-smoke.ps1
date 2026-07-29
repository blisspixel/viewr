#Requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Binary = "target/debug/viewr.exe",
    [ValidateRange(5, 120)]
    [int]$TimeoutSeconds = 60,
    [ValidateRange(30, 600)]
    [int]$SuiteTimeoutSeconds = 180
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
$script:SuiteDeadline = [DateTime]::UtcNow.AddSeconds($SuiteTimeoutSeconds)

if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw "the native UI Automation smoke test requires Windows"
}

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

if (-not ("ViewrAccessibilityNativeMethods" -as [type])) {
    Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class ViewrAccessibilityNativeMethods {
    public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)]
    public struct Rect {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int capacity);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hwnd);

    [DllImport("user32.dll")]
    public static extern bool PostMessage(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool GetClientRect(IntPtr hwnd, out Rect rect);

    public static int[] ClientSize(IntPtr hwnd) {
        Rect rect;
        if (!GetClientRect(hwnd, out rect)) {
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
        }
        return new[] { rect.Right - rect.Left, rect.Bottom - rect.Top };
    }

    public static IntPtr[] WindowsForProcess(uint processId) {
        var result = new List<IntPtr>();
        EnumWindows(delegate(IntPtr hwnd, IntPtr lParam) {
            uint owner;
            GetWindowThreadProcessId(hwnd, out owner);
            if (owner == processId) result.Add(hwnd);
            return true;
        }, IntPtr.Zero);
        return result.ToArray();
    }

    public static bool WindowBelongsToProcess(IntPtr hwnd, uint processId) {
        uint owner;
        GetWindowThreadProcessId(hwnd, out owner);
        return owner == processId;
    }

    public static string WindowText(IntPtr hwnd) {
        var text = new StringBuilder(512);
        GetWindowText(hwnd, text, text.Capacity);
        return text.ToString();
    }
}
"@
}

function Get-ApplicationWindow {
    foreach ($candidate in [ViewrAccessibilityNativeMethods]::WindowsForProcess($script:Process.Id)) {
        if (
            [ViewrAccessibilityNativeMethods]::IsWindowVisible($candidate) -and
            [ViewrAccessibilityNativeMethods]::WindowText($candidate) -eq "viewr"
        ) {
            return $candidate
        }
    }
    return [IntPtr]::Zero
}

function Get-ApplicationClientSize {
    $size = [ViewrAccessibilityNativeMethods]::ClientSize($script:Window)
    return [pscustomobject]@{
        Width = $size[0]
        Height = $size[1]
    }
}

function Wait-ForResult {
    param(
        [Parameter(Mandatory)]
        [scriptblock]$Probe,
        [Parameter(Mandatory)]
        [string]$Description
    )

    $operationDeadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $deadline = if ($operationDeadline -lt $script:SuiteDeadline) {
        $operationDeadline
    }
    else {
        $script:SuiteDeadline
    }
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($script:Process.HasExited) {
            throw "viewr exited before $Description (exit $($script:Process.ExitCode))"
        }
        try {
            $result = & $Probe
            if ($null -ne $result -and $result -ne [IntPtr]::Zero) {
                return $result
            }
        }
        catch [System.Windows.Automation.ElementNotAvailableException] {
            # The tree can be replaced between a query and an action. Retry from
            # the current root rather than treating a valid update as a failure.
        }
        Start-Sleep -Milliseconds 100
    }
    if ([DateTime]::UtcNow -ge $script:SuiteDeadline) {
        throw "accessibility smoke suite exceeded its $SuiteTimeoutSeconds second deadline"
    }
    $treeSummary = Get-TreeSummary
    throw "timed out waiting for $Description; accessible tree: $treeSummary"
}

function Get-TreeSummary {
    if ($script:Window -eq [IntPtr]::Zero) {
        return "window unavailable"
    }
    try {
        $root = [System.Windows.Automation.AutomationElement]::FromHandle($script:Window)
        $nodes = $root.FindAll(
            [System.Windows.Automation.TreeScope]::Subtree,
            [System.Windows.Automation.Condition]::TrueCondition
        )
        $names = [Collections.Generic.List[string]]::new()
        for ($index = 0; $index -lt $nodes.Count; $index++) {
            $name = $nodes.Item($index).Current.Name
            if (-not [string]::IsNullOrWhiteSpace($name)) {
                $names.Add($name)
            }
        }
        return $names -join ", "
    }
    catch {
        return "tree unavailable"
    }
}

function Get-Element {
    param(
        [Parameter(Mandatory)]
        [string]$Name,
        [System.Windows.Automation.ControlType]$ControlType,
        [switch]$Prefix
    )

    $root = [System.Windows.Automation.AutomationElement]::FromHandle($script:Window)
    $nodes = $root.FindAll(
        [System.Windows.Automation.TreeScope]::Subtree,
        [System.Windows.Automation.Condition]::TrueCondition
    )
    for ($index = 0; $index -lt $nodes.Count; $index++) {
        $node = $nodes.Item($index)
        $currentName = [string]$node.Current.Name
        $nameMatches = if ($Prefix) {
            $currentName.StartsWith($Name, [StringComparison]::Ordinal)
        }
        else {
            $currentName -eq $Name
        }
        $typeMatches = $null -eq $ControlType -or $node.Current.ControlType -eq $ControlType
        if ($nameMatches -and $typeMatches) {
            return $node
        }
    }
    return $null
}

function Wait-ForElement {
    param(
        [Parameter(Mandatory)]
        [string]$Name,
        [System.Windows.Automation.ControlType]$ControlType,
        [switch]$Prefix
    )

    return Wait-ForResult -Description "accessible element '$Name'" -Probe {
        Get-Element -Name $Name -ControlType $ControlType -Prefix:$Prefix
    }
}

function Wait-ForElementAbsent {
    param(
        [Parameter(Mandatory)]
        [string]$Name,
        [System.Windows.Automation.ControlType]$ControlType,
        [switch]$Prefix
    )

    return Wait-ForResult -Description "accessible element '$Name' to disappear" -Probe {
        $element = Get-Element -Name $Name -ControlType $ControlType -Prefix:$Prefix
        if ($null -eq $element) {
            return [IntPtr]1
        }
        return $null
    }
}

function Activate-Element {
    param(
        [Parameter(Mandatory)]
        [System.Windows.Automation.AutomationElement]$Element
    )

    $rawPattern = $null
    if ($Element.TryGetCurrentPattern(
        [System.Windows.Automation.InvokePattern]::Pattern,
        [ref]$rawPattern
    )) {
        $pattern = [System.Windows.Automation.InvokePattern]$rawPattern
        $pattern.Invoke()
        return
    }
    if ($Element.TryGetCurrentPattern(
        [System.Windows.Automation.TogglePattern]::Pattern,
        [ref]$rawPattern
    )) {
        $pattern = [System.Windows.Automation.TogglePattern]$rawPattern
        $pattern.Toggle()
        return
    }
    $supported = $Element.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName }
    throw (
        "accessible '$($Element.Current.Name)' cannot be activated; supported patterns: " +
        ($supported -join ", ")
    )
}

function Get-ToggleState {
    param(
        [Parameter(Mandatory)]
        [System.Windows.Automation.AutomationElement]$Element
    )

    $pattern = [System.Windows.Automation.TogglePattern]$Element.GetCurrentPattern(
        [System.Windows.Automation.TogglePattern]::Pattern
    )
    return $pattern.Current.ToggleState
}

function Toggle-Element {
    param(
        [Parameter(Mandatory)]
        [System.Windows.Automation.AutomationElement]$Element
    )

    $pattern = [System.Windows.Automation.TogglePattern]$Element.GetCurrentPattern(
        [System.Windows.Automation.TogglePattern]::Pattern
    )
    $pattern.Toggle()
}

function Wait-ForToggleState {
    param(
        [Parameter(Mandatory)]
        [string]$Name,
        [Parameter(Mandatory)]
        [System.Windows.Automation.ToggleState]$State
    )

    return Wait-ForResult -Description "'$Name' toggle state $State" -Probe {
        $element = Get-Element -Name $Name
        if ($null -eq $element) {
            return $null
        }
        $rawPattern = $null
        if (-not $element.TryGetCurrentPattern(
            [System.Windows.Automation.TogglePattern]::Pattern,
            [ref]$rawPattern
        )) {
            return $null
        }
        $pattern = [System.Windows.Automation.TogglePattern]$rawPattern
        if ($pattern.Current.ToggleState -eq $State) {
            return $element
        }
        return $null
    }
}

function Get-SelectionState {
    param(
        [Parameter(Mandatory)]
        [System.Windows.Automation.AutomationElement]$Element
    )

    $pattern = [System.Windows.Automation.SelectionItemPattern]$Element.GetCurrentPattern(
        [System.Windows.Automation.SelectionItemPattern]::Pattern
    )
    return $pattern.Current.IsSelected
}

function Select-Element {
    param(
        [Parameter(Mandatory)]
        [System.Windows.Automation.AutomationElement]$Element
    )

    $pattern = [System.Windows.Automation.SelectionItemPattern]$Element.GetCurrentPattern(
        [System.Windows.Automation.SelectionItemPattern]::Pattern
    )
    $pattern.Select()
}

function Wait-ForSelectionState {
    param(
        [Parameter(Mandatory)]
        [string]$Name,
        [Parameter(Mandatory)]
        [bool]$Selected,
        [switch]$Prefix
    )

    return Wait-ForResult -Description "'$Name' selected state $Selected" -Probe {
        $element = Get-Element -Name $Name -Prefix:$Prefix -ControlType (
            [System.Windows.Automation.ControlType]::RadioButton
        )
        if ($null -ne $element -and (Get-SelectionState -Element $element) -eq $Selected) {
            return $element
        }
        return $null
    }
}

function Open-ViewSubmenu {
    param(
        [Parameter(Mandatory)]
        [string]$Name
    )

    $view = Wait-ForElement -Name "View" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $view
    $submenu = Wait-ForElement -Name $Name -Prefix -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $submenu
}

function Stop-TestApplication {
    if ($null -eq $script:Process -or $script:Process.HasExited) {
        $script:Process = $null
        $script:Window = [IntPtr]::Zero
        return
    }
    if (
        $script:Window -ne [IntPtr]::Zero -and
        [ViewrAccessibilityNativeMethods]::WindowBelongsToProcess(
            $script:Window,
            [uint32]$script:Process.Id
        )
    ) {
        [void][ViewrAccessibilityNativeMethods]::PostMessage(
            $script:Window,
            0x0010,
            [IntPtr]::Zero,
            [IntPtr]::Zero
        )
    }
    if (-not $script:Process.WaitForExit(3000)) {
        $script:Process.Kill($true)
        $script:Process.WaitForExit()
    }
    $script:Process.Dispose()
    $script:Process = $null
    $script:Window = [IntPtr]::Zero
}

function Start-TestApplication {
    param([string]$ImagePath)

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $binaryPath
    $startInfo.UseShellExecute = $false
    if (-not [string]::IsNullOrWhiteSpace($ImagePath)) {
        $startInfo.ArgumentList.Add($ImagePath)
    }
    $startInfo.Environment["APPDATA"] = $testDirectory
    $script:Process = [Diagnostics.Process]::Start($startInfo)
    if ($null -eq $script:Process) {
        throw "viewr process did not start"
    }
    $script:Window = Wait-ForResult -Description "the visible viewr window" -Probe {
        Get-ApplicationWindow
    }
}

$binaryPath = (Resolve-Path -LiteralPath $Binary).Path
$versionMatch = [regex]::Match(
    (& $binaryPath --version | Out-String).Trim(),
    '^viewr (?<Version>\S+)$'
)
if (-not $versionMatch.Success) {
    throw "viewr --version did not return the expected local version contract"
}
$currentVersionText = "Current version: $($versionMatch.Groups['Version'].Value)"
$testDirectory = Join-Path (
    [IO.Path]::GetFullPath((Join-Path (Get-Location) "target"))
) "accessibility-smoke-$PID-$([Guid]::NewGuid().ToString('N'))"
$firstImage = Join-Path $testDirectory "first.png"
$secondImage = Join-Path $testDirectory "second.png"
$appearanceDirectory = Join-Path $testDirectory "viewr"
$appearanceFile = Join-Path $appearanceDirectory "appearance"
$png = [Convert]::FromBase64String(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
)

$script:Process = $null
$script:Window = [IntPtr]::Zero

try {
    [IO.Directory]::CreateDirectory($testDirectory) | Out-Null
    [IO.File]::WriteAllBytes($firstImage, $png)
    [IO.File]::WriteAllBytes($secondImage, $png)

    Start-TestApplication

    $root = Wait-ForResult -Description "the native AccessKit tree" -Probe {
        $candidate = [System.Windows.Automation.AutomationElement]::FromHandle($script:Window)
        if (
            $candidate.Current.Name -eq "viewr" -and
            $candidate.Current.ControlType -eq [System.Windows.Automation.ControlType]::Window
        ) {
            return $candidate
        }
        return $null
    }
    if (-not $root.Current.IsKeyboardFocusable) {
        throw "the native accessibility root is not keyboard-focusable"
    }
    $emptyWindowClientSize = Get-ApplicationClientSize

    foreach ($menu in @("File", "Edit", "Tools", "View", "Help")) {
        $element = Wait-ForElement -Name $menu -ControlType (
            [System.Windows.Automation.ControlType]::Button
        )
        if (-not $element.Current.IsKeyboardFocusable) {
            throw "the '$menu' menu is not keyboard-focusable"
        }
    }
    Wait-ForElement `
        -Name "Open a file to start. Its folder is browsed when access allows. Open Folder selects it explicitly for this session." `
        -ControlType ([System.Windows.Automation.ControlType]::Text) | Out-Null
    Wait-ForElement -Name "Open File" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    ) | Out-Null
    Wait-ForElement -Name "Open Folder" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    ) | Out-Null
    Wait-ForElement -Name "Local only. No cloud or viewr activity log." -ControlType (
        [System.Windows.Automation.ControlType]::Text
    ) | Out-Null

    Stop-TestApplication
    Start-TestApplication -ImagePath $firstImage

    $fileMenu = Wait-ForElement -Name "File" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $fileMenu
    $undoTrash = Wait-ForElement -Name "Undo Trash" -Prefix -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    if ($undoTrash.Current.Name -ne "Undo Trash") {
        throw "Undo Trash exposed a receipt count before a recoverable action existed"
    }
    if ($undoTrash.Current.IsEnabled) {
        throw "Undo Trash was enabled without a recoverable trash receipt"
    }
    $moveToTrash = Wait-ForElement -Name "Move to Trash" -Prefix -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    if (-not $moveToTrash.Current.IsEnabled) {
        throw "Move to Trash was disabled for a loaded image"
    }
    foreach ($obsoleteCullingAction in @(
        "Mark for batch trash",
        "Review next marked image",
        "Move 1 marked image",
        "Remove batch-trash mark"
    )) {
        Wait-ForElementAbsent `
            -Name $obsoleteCullingAction `
            -Prefix `
            -ControlType ([System.Windows.Automation.ControlType]::Button) | Out-Null
    }
    Activate-Element -Element $fileMenu
    Wait-ForElement -Name "first.png" -ControlType (
        [System.Windows.Automation.ControlType]::Text
    ) | Out-Null
    Wait-ForElement -Name "1 × 1" -ControlType (
        [System.Windows.Automation.ControlType]::Text
    ) | Out-Null
    $imageWindowClientSize = Get-ApplicationClientSize
    if (
        $imageWindowClientSize.Width -ne $emptyWindowClientSize.Width -or
        $imageWindowClientSize.Height -ne $emptyWindowClientSize.Height
    ) {
        throw (
            "opening the initial image resized the client area from " +
            "$($emptyWindowClientSize.Width)x$($emptyWindowClientSize.Height) to " +
            "$($imageWindowClientSize.Width)x$($imageWindowClientSize.Height)"
        )
    }
    Wait-ForElement -Name "1 / 2" -ControlType (
        [System.Windows.Automation.ControlType]::Text
    ) | Out-Null

    $help = Wait-ForElement -Name "Help" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $help
    $updateViewr = Wait-ForElement -Name "Update viewr..." -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $updateViewr
    Wait-ForElement -Name "Update viewr." -Prefix -ControlType (
        [System.Windows.Automation.ControlType]::Window
    ) | Out-Null
    foreach ($updateText in @(
        $currentVersionText,
        "viewr does not check, download, or install updates.",
        "No verified public update source is configured for this build.",
        "cargo build --release --workspace --locked"
    )) {
        Wait-ForElement -Name $updateText -ControlType (
            [System.Windows.Automation.ControlType]::Text
        ) | Out-Null
    }
    $closeUpdate = Wait-ForElement -Name "Close" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $closeUpdate
    Wait-ForElementAbsent -Name "Close" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    ) | Out-Null

    $help = Wait-ForElement -Name "Help" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $help
    $about = Wait-ForElement -Name "About viewr" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $about
    Wait-ForElement -Name "About viewr" -Prefix -ControlType (
        [System.Windows.Automation.ControlType]::Window
    ) | Out-Null
    $closeAbout = Wait-ForElement -Name "Close" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $closeAbout
    Wait-ForElementAbsent -Name "Close" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    ) | Out-Null

    Open-ViewSubmenu -Name "Appearance: System"
    Wait-ForElement `
        -Name "Changes app chrome and its default canvas. Image pixels stay unchanged; Image Background overrides the canvas separately." `
        -ControlType ([System.Windows.Automation.ControlType]::Text) | Out-Null
    Wait-ForSelectionState -Name "System:" -Prefix -Selected $true | Out-Null
    $lightTheme = Wait-ForSelectionState -Name "Light:" -Prefix -Selected $false
    Select-Element -Element $lightTheme
    Open-ViewSubmenu -Name "Appearance: Light"
    Wait-ForSelectionState -Name "Light:" -Prefix -Selected $true | Out-Null
    $darkTheme = Wait-ForSelectionState -Name "Dark:" -Prefix -Selected $false
    Select-Element -Element $darkTheme
    Open-ViewSubmenu -Name "Appearance: Dark"
    Wait-ForSelectionState -Name "Dark:" -Prefix -Selected $true | Out-Null
    $consoleTheme = Wait-ForSelectionState -Name "Console:" -Prefix -Selected $false
    Select-Element -Element $consoleTheme
    Open-ViewSubmenu -Name "Appearance: Console"
    Wait-ForSelectionState -Name "Console:" -Prefix -Selected $true | Out-Null
    $systemTheme = Wait-ForSelectionState -Name "System:" -Prefix -Selected $false
    Select-Element -Element $systemTheme
    Open-ViewSubmenu -Name "Appearance: System"
    Wait-ForSelectionState `
        -Name "System: Follows your operating system. Currently " `
        -Prefix `
        -Selected $true | Out-Null
    $consoleTheme = Wait-ForSelectionState -Name "Console:" -Prefix -Selected $false
    Select-Element -Element $consoleTheme
    Open-ViewSubmenu -Name "Appearance: Console"
    Wait-ForSelectionState -Name "Console:" -Prefix -Selected $true | Out-Null
    if (-not [IO.File]::Exists($appearanceFile)) {
        throw "selecting Console did not persist the isolated appearance preference"
    }
    $appearanceValue = [IO.File]::ReadAllText($appearanceFile)
    if (-not [string]::Equals($appearanceValue, "console`n", [StringComparison]::Ordinal)) {
        throw "Console preference file did not contain the exact validated value"
    }

    Stop-TestApplication
    Start-TestApplication -ImagePath $firstImage
    Wait-ForElement -Name "first.png" -ControlType (
        [System.Windows.Automation.ControlType]::Text
    ) | Out-Null
    Open-ViewSubmenu -Name "Appearance: Console"
    Wait-ForSelectionState -Name "Console:" -Prefix -Selected $true | Out-Null
    $fileMenu = Wait-ForElement -Name "File" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $fileMenu
    $openWith = Wait-ForElement -Name "Open With..." -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    if (-not $openWith.Current.IsEnabled) {
        throw "Open With was not enabled for the accepted current image"
    }

    Open-ViewSubmenu -Name "Panels"
    foreach ($panel in @("Tools T", "Folder Previews G", "Image Information I")) {
        Wait-ForToggleState -Name $panel -State (
            [System.Windows.Automation.ToggleState]::Off
        ) | Out-Null
    }

    $tools = Wait-ForToggleState -Name "Tools T" -State (
        [System.Windows.Automation.ToggleState]::Off
    )
    Toggle-Element -Element $tools
    $collapseTools = Wait-ForElement -Name "Collapse tools panel" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Wait-ForElement -Name "Rotate clockwise (R)" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    ) | Out-Null
    Wait-ForElementAbsent `
        -Name "Mark for batch trash" `
        -Prefix `
        -ControlType ([System.Windows.Automation.ControlType]::Button) | Out-Null
    $editMenu = Wait-ForElement -Name "Edit" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $editMenu
    $startCrop = Wait-ForElement -Name "Crop" -Prefix -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $startCrop
    $cancelCrop = Wait-ForElement -Name "Cancel" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $cancelCrop
    $spotHeal = Wait-ForElement -Name "Spot heal (J)" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $spotHeal
    Wait-ForElement -Name "Brush radius" -ControlType (
        [System.Windows.Automation.ControlType]::Slider
    ) | Out-Null
    Wait-ForElement -Name "Feather" -ControlType (
        [System.Windows.Automation.ControlType]::Slider
    ) | Out-Null
    Wait-ForElement -Name "Refresh source" -Prefix -ControlType (
        [System.Windows.Automation.ControlType]::Button
    ) | Out-Null
    $finishSpotHeal = Wait-ForElement -Name "Done" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $finishSpotHeal
    Wait-ForElementAbsent -Name "Done" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    ) | Out-Null
    $collapseTools = Wait-ForElement -Name "Collapse tools panel" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $collapseTools
    Wait-ForElement -Name "Expand tools panel" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    ) | Out-Null

    Open-ViewSubmenu -Name "Panels"
    Wait-ForToggleState -Name "Tools T" -State (
        [System.Windows.Automation.ToggleState]::On
    ) | Out-Null
    $information = Wait-ForToggleState -Name "Image Information I" -State (
        [System.Windows.Automation.ToggleState]::Off
    )
    Toggle-Element -Element $information

    Wait-ForElement -Name "Source Privacy" -ControlType (
        [System.Windows.Automation.ControlType]::Text
    ) | Out-Null
    Wait-ForElement -Name "No supported EXIF detected." -ControlType (
        [System.Windows.Automation.ControlType]::Text
    ) | Out-Null
    Wait-ForElement `
        -Name "Limited EXIF scan. Other metadata or hidden pixel data may still exist." `
        -ControlType ([System.Windows.Automation.ControlType]::Text) | Out-Null

    $metadata = Wait-ForToggleState -Name "Keep camera metadata when saving" -State (
        [System.Windows.Automation.ToggleState]::Off
    )
    Toggle-Element -Element $metadata
    Wait-ForToggleState -Name "Keep camera metadata when saving" -State (
        [System.Windows.Automation.ToggleState]::On
    ) | Out-Null

    Open-ViewSubmenu -Name "Panel Position"
    Wait-ForSelectionState -Name "Tools: Left" -Selected $true | Out-Null
    $toolsRight = Wait-ForSelectionState -Name "Tools: Right" -Selected $false
    Wait-ForSelectionState -Name "Image Information: Left" -Selected $false | Out-Null
    Wait-ForSelectionState -Name "Image Information: Right" -Selected $true | Out-Null
    Select-Element -Element $toolsRight

    Open-ViewSubmenu -Name "Panel Position"
    Wait-ForSelectionState -Name "Tools: Right" -Selected $true | Out-Null
    $informationLeft = Wait-ForSelectionState `
        -Name "Image Information: Left" `
        -Selected $false
    Select-Element -Element $informationLeft

    Open-ViewSubmenu -Name "Panel Position"
    Wait-ForSelectionState -Name "Image Information: Left" -Selected $true | Out-Null
    $view = Wait-ForElement -Name "View" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $view

    Open-ViewSubmenu -Name "Panels"
    $previews = Wait-ForToggleState -Name "Folder Previews G" -State (
        [System.Windows.Automation.ToggleState]::Off
    )
    Toggle-Element -Element $previews
    Wait-ForElement -Name "Collapse folder previews" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    ) | Out-Null
    Wait-ForElement -Name "image 1: first.png" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    ) | Out-Null
    $secondThumbnail = Wait-ForElement -Name "image 2: second.png" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Activate-Element -Element $secondThumbnail
    Wait-ForElement -Name "2 / 2" -ControlType (
        [System.Windows.Automation.ControlType]::Text
    ) | Out-Null

    Write-Output (
        "accessibility-smoke: PASS; native UIA tree, focusability, panel state, " +
        "actions, first-run scope, stable initial window size, conventional Trash " +
        "controls, local update guidance, About, current appearance and restart, " +
        "Spot Heal, source privacy, native Open With discovery, panel shortcuts, dock positions, " +
        "metadata state, disabled trash recovery, previews, and navigation verified"
    )
}
finally {
    Stop-TestApplication
    foreach ($path in @($firstImage, $secondImage)) {
        if ([IO.File]::Exists($path)) {
            [IO.File]::Delete($path)
        }
    }
    if ([IO.File]::Exists($appearanceFile)) {
        [IO.File]::Delete($appearanceFile)
    }
    if ([IO.Directory]::Exists($appearanceDirectory)) {
        [IO.Directory]::Delete($appearanceDirectory, $false)
    }
    if ([IO.Directory]::Exists($testDirectory)) {
        [IO.Directory]::Delete($testDirectory, $false)
    }
}
