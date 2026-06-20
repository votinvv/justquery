param(
    [Parameter(Mandatory=$true)][int]$X,
    [Parameter(Mandatory=$true)][int]$Y,
    [string]$Proc = "justquery"
)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class U {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, IntPtr i);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    public const uint LEFTDOWN = 0x02, LEFTUP = 0x04;
}
"@
$proc = Get-Process -Name $Proc -ErrorAction SilentlyContinue | Select-Object -First 1
if ($proc) { [U]::SetForegroundWindow($proc.MainWindowHandle) | Out-Null }
Start-Sleep -Milliseconds 200
[U]::SetCursorPos($X, $Y) | Out-Null
Start-Sleep -Milliseconds 80
[U]::mouse_event([U]::LEFTDOWN, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 40
[U]::mouse_event([U]::LEFTUP, 0, 0, 0, [IntPtr]::Zero)
Write-Output "clicked ($X,$Y)"
