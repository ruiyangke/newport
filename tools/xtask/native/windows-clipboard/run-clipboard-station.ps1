[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$TestExecutable,
    [Parameter(Mandatory = $true)][ValidatePattern('^[a-fA-F0-9]{64}$')][string]$ExpectedSha256,
    [Parameter(Mandatory = $true)][ValidateSet('static', 'render', 'privacy', 'destroy')][string]$Mode,
    [string]$StaticTest = '',
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [ValidateRange(15, 120)][int]$TimeoutSeconds = 45,
    [ValidateRange(1, 2147483647)][int]$UserSessionId,
    [string]$ExpectedUserSid = ''
)
$ErrorActionPreference = 'Stop'
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'This harness runs only on the selected Windows test VM.' }
$broker = $PSBoundParameters.ContainsKey('UserSessionId')
if ($broker -ne (-not [String]::IsNullOrEmpty($ExpectedUserSid))) { throw 'Broker mode requires an explicit session and expected user SID.' }
if (-not [IO.Path]::IsPathRooted($OutputDirectory)) { throw 'OutputDirectory must be absolute.' }
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a new output directory for every native attempt.' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$directory = New-Item -ItemType Directory -Path $OutputDirectory
$acl = Get-Acl -LiteralPath $OutputDirectory
$acl.SetAccessRuleProtection($true, $false)
$user = [Security.Principal.WindowsIdentity]::GetCurrent().User
$system = New-Object Security.Principal.SecurityIdentifier 'S-1-5-18'
foreach ($sid in @($user, $system)) {
    $rule = New-Object Security.AccessControl.FileSystemAccessRule($sid, 'FullControl', 'ContainerInherit, ObjectInherit', 'None', 'Allow')
    $acl.AddAccessRule($rule)
}
Set-Acl -LiteralPath $OutputDirectory -AclObject $acl
$source = Join-Path $PSScriptRoot 'clipboard-station.cs'
try {
    if ($PSVersionTable.PSEdition -eq 'Desktop') {
        Add-Type -Path $source -ReferencedAssemblies @('System.dll', 'System.Core.dll')
    } else {
        Add-Type -Path $source
    }
    if ($broker) {
        $result = [ClipboardStationHarness]::RunAsUser($TestExecutable, $ExpectedSha256, $Mode, $StaticTest, $OutputDirectory, $TimeoutSeconds, [uint32]$UserSessionId, $ExpectedUserSid)
    } else {
        $result = [ClipboardStationHarness]::Run($TestExecutable, $ExpectedSha256, $Mode, $StaticTest, $OutputDirectory, $TimeoutSeconds)
    }
    $receipt = Join-Path $OutputDirectory 'receipt.json'
    $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $receipt -Encoding UTF8
    [pscustomobject]@{ Passed = $result.Passed; Receipt = $receipt; Error = $result.Error }
    if (-not $result.Passed) { exit 1 }
} catch {
    [pscustomobject]@{ Passed = $false; Stage = 'harness'; Error = $_.Exception.Message } |
        ConvertTo-Json | Set-Content -LiteralPath (Join-Path $OutputDirectory 'harness-error.json') -Encoding UTF8
    throw
}
