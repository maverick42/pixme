# Autonomous probe for the Windows full-screen pick overlay.
# Launches pixme with PIXME_PICK_TEST=1, moves the OS cursor, scans a
# crop around the cursor for the loupe's red crosshair (240,70,70),
# checks the pick HWND + foreground, sends Esc, verifies the overlay
# went away. Always kills pixme at the end.
param(
    [string]$Exe = "C:\projets\rust\pixme\target\debug\pixme.exe",
    [string]$Log = "C:\projets\rust\pixme\pick_probe.log",
    [int]$SettleMs = 900,
    [switch]$SendClick
)
$ErrorActionPreference = 'Continue'

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class U32 {
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
    public struct POINT { public int X; public int Y; }
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    public static System.Collections.Generic.List<string> TitlesOfPid(uint want) {
        var list = new System.Collections.Generic.List<string>();
        EnumWindows((h, l) => {
            uint pid; GetWindowThreadProcessId(h, out pid);
            if (pid == want) {
                var sb = new StringBuilder(96);
                GetWindowTextW(h, sb, 96);
                list.Add(string.Format("{0} t='{1}' v={2}", h, sb, IsWindowVisible(h)));
            }
            return true;
        }, IntPtr.Zero);
        return list;
    }
}
"@

function Get-ForegroundTitle {
    $h = [U32]::GetForegroundWindow()
    $sb = New-Object System.Text.StringBuilder 128
    [void][U32]::GetWindowTextW($h, $sb, 128)
    return $sb.ToString()
}

function Probe-LoupeNearCursor($cx, $cy, $pngPath) {
    # Crop a square around the cursor and count loupe-crosshair pixels.
    $vs = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $size = 320
    $x0 = [math]::Max($vs.Left, [math]::Min($cx - $size/2, $vs.Right - $size))
    $y0 = [math]::Max($vs.Top, [math]::Min($cy - $size/2, $vs.Bottom - $size))
    $bmp = New-Object System.Drawing.Bitmap $size, $size
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen([int]$x0, [int]$y0, 0, 0, $bmp.Size)
    $g.Dispose()
    if ($pngPath) { $bmp.Save($pngPath, [System.Drawing.Imaging.ImageFormat]::Png) }
    $rect = New-Object System.Drawing.Rectangle 0, 0, $bmp.Width, $bmp.Height
    $data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly,
        [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $bytes = New-Object byte[] ($data.Stride * $data.Height)
    [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $bytes, 0, $bytes.Length)
    $bmp.UnlockBits($data)
    $hits = 0
    for ($y = 0; $y -lt $data.Height; $y++) {
        $row = $y * $data.Stride
        for ($x = 0; $x -lt $data.Width; $x++) {
            $i = $row + $x * 4
            if ([math]::Abs([int]$bytes[$i+2] - 240) -lt 28 -and
                [math]::Abs([int]$bytes[$i+1] - 70) -lt 28 -and
                [math]::Abs([int]$bytes[$i] - 70) -lt 28) { $hits++ }
        }
    }
    $bmp.Dispose()
    return $hits
}

Get-Process -Name pixme -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400

$env:PIXME_PICK_TEST = '1'
$env:RUST_LOG = 'pixme=debug'
$p = Start-Process -FilePath $Exe -WorkingDirectory (Split-Path $Exe -Parent) -PassThru `
    -RedirectStandardOutput "$Log.out" -RedirectStandardError $Log
Write-Output "PID $($p.Id) launched (PIXME_PICK_TEST=1)"
Start-Sleep -Milliseconds 2500

try {
    $vs = [System.Windows.Forms.SystemInformation]::VirtualScreen
    Write-Output ("virtual screen: {0},{1} {2}x{3}" -f $vs.Left, $vs.Top, $vs.Width, $vs.Height)
    $xs = @(1, 1, ($vs.Right - 2), 400)
    $ys = @(1, ($vs.Bottom - 2), ($vs.Bottom - 2), 60)
    $ok = $true
    for ($i = 0; $i -lt $xs.Count; $i++) {
        [void][U32]::SetCursorPos($xs[$i], $ys[$i])
        Start-Sleep -Milliseconds $SettleMs
        $png = "C:\projets\rust\pixme\pick_crop_$i.png"
        $hits = Probe-LoupeNearCursor $xs[$i] $ys[$i] $png
        Write-Output ("cursor ({0},{1}) -> loupe hits={2} crop={3}" -f $xs[$i], $ys[$i], $hits, $png)
        if ($hits -lt 100) { $ok = $false }
    }
    foreach ($w in [U32]::TitlesOfPid([uint32]$p.Id)) { Write-Output "  hwnd: $w" }
    $pickHwnd = [U32]::FindWindowW($null, 'PixMe Pick')
    Write-Output ("foreground='{0}' pickHwnd={1} visible={2}" -f (Get-ForegroundTitle), $pickHwnd, [U32]::IsWindowVisible($pickHwnd))
    Write-Output ("LOUPE_VISIBLE={0}" -f $ok)

    if ($SendClick) {
        # A left click must validate the pick: the overlay closes and the
        # press must NOT reach the app beneath (it lands on our window).
        [void][U32]::SetCursorPos(400, 60)
        Start-Sleep -Milliseconds 400
        [U32]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)   # LEFTDOWN
        Start-Sleep -Milliseconds 40
        [U32]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)   # LEFTUP
        Start-Sleep -Milliseconds 900
        $left = Probe-LoupeNearCursor 400 60 $null
        Write-Output ("after click: loupe hits={0} foreground='{1}'" -f $left, (Get-ForegroundTitle))
        Write-Output ("CLICK_PICKED={0}" -f ($left -le 100))
    } else {
        # Esc must cancel the pick (overlay closes, pick_test_done logs).
        [U32]::keybd_event(0x1B, 0, 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 60
        [U32]::keybd_event(0x1B, 0, 0x0002, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 900
        $left = Probe-LoupeNearCursor 400 60 $null
        Write-Output ("after Esc: loupe hits={0} foreground='{1}'" -f $left, (Get-ForegroundTitle))
        Write-Output ("ESC_CANCELLED={0}" -f ($left -le 100))
        # The pick HWND may linger while the anchor sleeps — what matters
        # is that it no longer swallows clicks: WindowFromPoint under the
        # cursor must return a NON-pixme window.
        $pt = New-Object U32+POINT
        $pt.X = 1; $pt.Y = 1
        $under = [U32]::WindowFromPoint($pt)
        $underPid = 0
        [void][U32]::GetWindowThreadProcessId($under, [ref]$underPid)
        Write-Output ("WindowFromPoint hwnd={0} pid={1} (pixme={2})" -f $under, $underPid, $p.Id)
        Write-Output ("CLICKS_FREE={0}" -f ($underPid -ne $p.Id))
        # The transparent HWND must be DESTROYED — a lingering invisible
        # window would keep swallowing every click on the screen. Before
        # Esc two 'P'-titled windows are visible (pick overlay + viewer);
        # after it only the viewer may remain.
        $pvis = 0
        foreach ($w in [U32]::TitlesOfPid([uint32]$p.Id)) {
            Write-Output "  hwnd-after: $w"
            if ($w -match "t='P' v=True") { $pvis++ }
        }
        Write-Output ("PICK_HWND_GONE={0}" -f ($pvis -le 1))
    }
}
finally {
    Get-Process -Name pixme -ErrorAction SilentlyContinue | Stop-Process -Force
}
