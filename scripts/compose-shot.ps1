# Stitches two screenshots into the README layout: satellite in the top-left
# half, carmine in the bottom-right, divided by a neon line with the same halo
# as button hover (accent glow, 16px).
#
#   powershell -File scripts/compose-shot.ps1 `
#     -Satellite shot-satellite.png -Carmine shot-carmine.png -Out screenshots/zapret.png
#
# Keep this file ASCII-only: Windows PowerShell 5.1 reads a BOM-less script as
# ANSI, and non-ASCII string literals break the parser.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Satellite,
    [Parameter(Mandatory = $true)][string]$Carmine,
    [Parameter(Mandatory = $true)][string]$Out
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$code = @"
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;

public class NeonCompose {
    private const int SATELLITE = 0x57d7ff; // --accent of the satellite theme
    private const int CARMINE = 0xe81c5a;   // --accent of the carmine theme

    private static double Channel(int color, int shift) {
        return ((color >> shift) & 0xFF) / 255.0;
    }

    public static void Diagonal(string satellite, string carmine, string output) {
        using (var top = new Bitmap(satellite))
        using (var bottom = new Bitmap(carmine)) {
            if (top.Width != bottom.Width || top.Height != bottom.Height) {
                throw new ArgumentException("Screenshots must have the same size.");
            }

            int w = top.Width, h = top.Height;
            using (var result = new Bitmap(w, h, PixelFormat.Format32bppArgb)) {
                var rect = new Rectangle(0, 0, w, h);
                var srcTop = top.LockBits(rect, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
                var srcBottom = bottom.LockBits(rect, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
                var dst = result.LockBits(rect, ImageLockMode.WriteOnly, PixelFormat.Format32bppArgb);

                int bytes = Math.Abs(srcTop.Stride) * h;
                byte[] a = new byte[bytes];
                byte[] b = new byte[bytes];
                byte[] o = new byte[bytes];
                Marshal.Copy(srcTop.Scan0, a, 0, bytes);
                Marshal.Copy(srcBottom.Scan0, b, 0, bytes);

                int stride = srcTop.Stride;
                double length = Math.Sqrt((double)w * w + (double)h * h);

                for (int y = 0; y < h; y++) {
                    for (int x = 0; x < w; x++) {
                        int i = y * stride + x * 4;
                        // Signed distance to the bottom-left -> top-right diagonal.
                        double d = (h * x + w * y - w * h) / length;
                        double upper = d < 0 ? 1.0 : 0.0;

                        // Tint runs from carmine at the bottom-left end to
                        // satellite at the top-right end.
                        double t = (double)x / (w - 1);
                        double r = Channel(CARMINE, 16) + (Channel(SATELLITE, 16) - Channel(CARMINE, 16)) * t;
                        double g = Channel(CARMINE, 8) + (Channel(SATELLITE, 8) - Channel(CARMINE, 8)) * t;
                        double bl = Channel(CARMINE, 0) + (Channel(SATELLITE, 0) - Channel(CARMINE, 0)) * t;

                        double core = Math.Exp(-(d * d) / (2 * 0.9 * 0.9)) * 0.9;
                        double glow = Math.Exp(-(d * d) / (2 * 4.0 * 4.0)) * 0.4;
                        double halo = Math.Exp(-(d * d) / (2 * 10.0 * 10.0)) * 0.18;
                        double alpha = Math.Min(0.95, core + glow + halo);

                        // Format32bppArgb stores bytes as B, G, R, A.
                        double baseB = (upper * a[i] + (1 - upper) * b[i]) / 255.0;
                        double baseG = (upper * a[i + 1] + (1 - upper) * b[i + 1]) / 255.0;
                        double baseR = (upper * a[i + 2] + (1 - upper) * b[i + 2]) / 255.0;

                        o[i] = (byte)Math.Round((baseB * (1 - alpha) + bl * alpha) * 255);
                        o[i + 1] = (byte)Math.Round((baseG * (1 - alpha) + g * alpha) * 255);
                        o[i + 2] = (byte)Math.Round((baseR * (1 - alpha) + r * alpha) * 255);
                        o[i + 3] = 255;
                    }
                }

                Marshal.Copy(o, 0, dst.Scan0, bytes);
                top.UnlockBits(srcTop);
                bottom.UnlockBits(srcBottom);
                result.UnlockBits(dst);
                result.Save(output, ImageFormat.Png);
            }
        }
    }
}
"@

Add-Type -TypeDefinition $code -ReferencedAssemblies System.Drawing
[NeonCompose]::Diagonal((Resolve-Path $Satellite).Path, (Resolve-Path $Carmine).Path, $Out)
Write-Host "Screenshot composed: $Out"
