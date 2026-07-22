#Requires -Version 7.0

[CmdletBinding()]
param(
    [string]$Binary = "target/debug/viewr.exe",
    [ValidateRange(5, 120)]
    [int]$TimeoutSeconds = 30
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

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

function Wait-ForResult {
    param(
        [Parameter(Mandatory)]
        [scriptblock]$Probe,
        [Parameter(Mandatory)]
        [string]$Description
    )

    while ([DateTime]::UtcNow -lt $script:Deadline) {
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
        $nameMatches = if ($Prefix) {
            $node.Current.Name.StartsWith($Name, [StringComparison]::Ordinal)
        }
        else {
            $node.Current.Name -eq $Name
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
        $element = Get-Element -Name $Name -ControlType (
            [System.Windows.Automation.ControlType]::CheckBox
        )
        if ($null -ne $element -and (Get-ToggleState -Element $element) -eq $State) {
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
        [bool]$Selected
    )

    return Wait-ForResult -Description "'$Name' selected state $Selected" -Probe {
        $element = Get-Element -Name $Name -ControlType (
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

$binaryPath = (Resolve-Path -LiteralPath $Binary).Path
$testDirectory = Join-Path (
    [IO.Path]::GetFullPath((Join-Path (Get-Location) "target"))
) "accessibility-smoke-$PID-$([Guid]::NewGuid().ToString('N'))"
$firstImage = Join-Path $testDirectory "first.png"
$secondImage = Join-Path $testDirectory "second.png"
$png = [Convert]::FromBase64String(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
)

$script:Process = $null
$script:Window = [IntPtr]::Zero
$script:Deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)

try {
    [IO.Directory]::CreateDirectory($testDirectory) | Out-Null
    [IO.File]::WriteAllBytes($firstImage, $png)
    [IO.File]::WriteAllBytes($secondImage, $png)

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $binaryPath
    $startInfo.UseShellExecute = $false
    $startInfo.ArgumentList.Add($firstImage)
    $script:Process = [Diagnostics.Process]::Start($startInfo)
    $script:Window = Wait-ForResult -Description "the visible viewr window" -Probe {
        Get-ApplicationWindow
    }

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

    foreach ($menu in @("File", "Edit", "View")) {
        $element = Wait-ForElement -Name $menu -ControlType (
            [System.Windows.Automation.ControlType]::Button
        )
        if (-not $element.Current.IsKeyboardFocusable) {
            throw "the '$menu' menu is not keyboard-focusable"
        }
    }
    Wait-ForElement -Name "first.png" -ControlType (
        [System.Windows.Automation.ControlType]::Text
    ) | Out-Null
    Wait-ForElement -Name "1 × 1" -ControlType (
        [System.Windows.Automation.ControlType]::Text
    ) | Out-Null
    Wait-ForElement -Name "1 / 2" -ControlType (
        [System.Windows.Automation.ControlType]::Text
    ) | Out-Null

    Open-ViewSubmenu -Name "Panels"
    foreach ($panel in @("Tools", "Folder Previews", "Image Information")) {
        Wait-ForToggleState -Name $panel -State (
            [System.Windows.Automation.ToggleState]::Off
        ) | Out-Null
    }

    $tools = Get-Element -Name "Tools" -ControlType (
        [System.Windows.Automation.ControlType]::CheckBox
    )
    Toggle-Element -Element $tools
    $collapseTools = Wait-ForElement -Name "Collapse tools panel" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    )
    Wait-ForElement -Name "Rotate clockwise (R)" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    ) | Out-Null
    Activate-Element -Element $collapseTools
    Wait-ForElement -Name "Expand tools panel" -ControlType (
        [System.Windows.Automation.ControlType]::Button
    ) | Out-Null

    Open-ViewSubmenu -Name "Panels"
    Wait-ForToggleState -Name "Tools" -State (
        [System.Windows.Automation.ToggleState]::On
    ) | Out-Null
    $information = Get-Element -Name "Image Information" -ControlType (
        [System.Windows.Automation.ControlType]::CheckBox
    )
    Toggle-Element -Element $information

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
    $previews = Wait-ForToggleState -Name "Folder Previews" -State (
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
        "actions, dock positions, metadata state, previews, and navigation verified"
    )
}
finally {
    if ($null -ne $script:Process -and -not $script:Process.HasExited) {
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
    }
    foreach ($path in @($firstImage, $secondImage)) {
        if ([IO.File]::Exists($path)) {
            [IO.File]::Delete($path)
        }
    }
    if ([IO.Directory]::Exists($testDirectory)) {
        [IO.Directory]::Delete($testDirectory, $false)
    }
}
