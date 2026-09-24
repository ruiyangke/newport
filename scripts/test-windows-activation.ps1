# Run on an interactive Windows desktop against a built Porthop executable.
param([Parameter(Mandatory=$true)][string]$BinaryPath)
$BinaryPath=(Resolve-Path $BinaryPath).Path
$ErrorActionPreference='Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ActivationTest {
 public delegate bool EnumProc(IntPtr h, IntPtr p);
 [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, System.Text.StringBuilder text, int size);
 public static IntPtr Find(uint pid) { IntPtr found=IntPtr.Zero; EnumWindows((h,l)=> {uint p;GetWindowThreadProcessId(h,out p); var text=new System.Text.StringBuilder(256);GetWindowText(h,text,256); if(p==pid && IsWindowVisible(h) && text.ToString()=="Porthop") {found=h;return false;}return true;}, IntPtr.Zero);return found; }
}
'@
$profile=Join-Path $env:TEMP ('PorthopActivation-'+[guid]::NewGuid())
New-Item -ItemType Directory $profile | Out-Null
$key=Join-Path $profile 'test-key'
$bytes=New-Object byte[] 32
$rng=[System.Security.Cryptography.RandomNumberGenerator]::Create()
$rng.GetBytes($bytes)
$rng.Dispose()
[IO.File]::WriteAllBytes($key,$bytes)
$env:PORTHOP_DATA_DIR=$profile
$env:PORTHOP_TEST_VAULT_KEY_FILE=$key
$first=$null
try {
 $first=Start-Process $BinaryPath -PassThru -RedirectStandardError (Join-Path $profile 'primary.log')
 $handle=[IntPtr]::Zero
 for($i=0;$i -lt 150;$i++) { Start-Sleep -Milliseconds 200; $handle=[ActivationTest]::Find($first.Id);if($handle -ne [IntPtr]::Zero){break} }
 if($handle -eq [IntPtr]::Zero){throw 'Initial window did not appear'}
 Start-Sleep -Seconds 3
 $handle=[ActivationTest]::Find($first.Id)
 [ActivationTest]::PostMessage($handle,0x0010,[IntPtr]::Zero,[IntPtr]::Zero) | Out-Null
 for($i=0;$i -lt 50;$i++){if(![ActivationTest]::IsWindowVisible($handle)){break};Start-Sleep -Milliseconds 100}
 if([ActivationTest]::IsWindowVisible($handle)){throw 'Window was not hidden'}
 $second=Start-Process $BinaryPath -PassThru -RedirectStandardError (Join-Path $profile 'secondary.log')
 $null=$second.Handle
 if(!$second.WaitForExit(10000)){Stop-Process -Id $second.Id;throw 'Second process did not exit'}
 for($i=0;$i -lt 50;$i++){if([ActivationTest]::IsWindowVisible($handle)){break};Start-Sleep -Milliseconds 100}
 if(![ActivationTest]::IsWindowVisible($handle)){throw 'Existing window was not reopened'}
 if($second.ExitCode -ne 0){throw 'Second process failed'}
 Write-Output 'PASS: hidden existing window reopened; second process exited successfully.'
} finally {
 if($first -and !$first.HasExited){Stop-Process -Id $first.Id -Force}
 Start-Sleep -Milliseconds 500
 Remove-Item $profile -Recurse -Force -ErrorAction SilentlyContinue
}
