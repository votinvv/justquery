param(
    [Parameter(Mandatory=$true)][string]$Out,
    [string]$Window = $null   # optional: capture this process's window instead of full screen
)
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

$bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
if ($Window) {
    $proc = Get-Process -Name $Window -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($proc -and $proc.MainWindowHandle -ne [IntPtr]::Zero) {
        Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W {
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
}
"@ -ErrorAction SilentlyContinue
        $r = New-Object W+RECT
        [W]::GetWindowRect($proc.MainWindowHandle, [ref]$r) | Out-Null
        $x = [Math]::Min($r.L, $r.R); $y = [Math]::Min($r.T, $r.B)
        $w = [Math]::Abs($r.R - $r.L); $h = [Math]::Abs($r.B - $r.T)
        $bounds = New-Object System.Drawing.Rectangle($x, $y, $w, $h)
    }
}
$bmp = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "saved $Out ($($bounds.Width)x$($bounds.Height))"
