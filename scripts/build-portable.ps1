param(
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$tauriRoot = Join-Path $repoRoot "src-tauri"
$configPath = Join-Path $tauriRoot "tauri.conf.json"
$releaseRoot = Join-Path $tauriRoot "target\release"
$binaryPath = Join-Path $releaseRoot "otr.exe"

Push-Location $repoRoot
try {
    if (-not $SkipBuild) {
        & npm run tauri -- build --no-bundle
        if ($LASTEXITCODE -ne 0) {
            throw "Tauri portable binary build failed with exit code $LASTEXITCODE"
        }
    }

    $config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
    $version = [string]$config.version
    if ([string]::IsNullOrWhiteSpace($version)) {
        throw "Missing version in $configPath"
    }
    if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
        throw "Release binary not found: $binaryPath"
    }

    $portableRoot = Join-Path $releaseRoot "bundle\portable"
    [System.IO.Directory]::CreateDirectory($portableRoot) | Out-Null
    $zipPath = Join-Path $portableRoot "OTR_${version}_x64-portable.zip"
    if (Test-Path -LiteralPath $zipPath) {
        for ($attempt = 1; $attempt -le 10; $attempt++) {
            try {
                Remove-Item -LiteralPath $zipPath -Force
                break
            }
            catch {
                if ($attempt -eq 10) {
                    throw "Portable archive is open in another application. Close it and retry: $zipPath"
                }
                Start-Sleep -Milliseconds 500
            }
        }
    }

    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $stream = [System.IO.File]::Open(
        $zipPath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::ReadWrite,
        [System.IO.FileShare]::None
    )
    try {
        $archive = [System.IO.Compression.ZipArchive]::new(
            $stream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $false,
            [System.Text.Encoding]::UTF8
        )
        try {
            [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $archive,
                $binaryPath,
                "OTR.exe",
                [System.IO.Compression.CompressionLevel]::Optimal
            ) | Out-Null

            $instructions = @"
OTR 便携版 (v$version)
=====================

Otae's Token Radar — 你电脑上所有 AI Coding Agent 的 Token 消耗雷达。

使用方法：直接双击 OTR.exe 运行，无需安装。

要求：
- Windows 10/11，且系统已安装 Microsoft Edge WebView2 运行时。
- Win10/11 通常自带；若启动白屏，请安装 WebView2：
  https://developer.microsoft.com/microsoft-edge/webview2/

数据位置：
- 用量数据与设置保存在 %APPDATA%\com.otae.radar。
- 便携版与安装版数据互通；删除该目录即可清除本地数据。
"@
            $entry = $archive.CreateEntry(
                "使用说明.txt",
                [System.IO.Compression.CompressionLevel]::Optimal
            )
            $writer = [System.IO.StreamWriter]::new(
                $entry.Open(),
                [System.Text.UTF8Encoding]::new($true)
            )
            try {
                $writer.Write($instructions)
            }
            finally {
                $writer.Dispose()
            }
        }
        finally {
            $archive.Dispose()
        }
    }
    finally {
        $stream.Dispose()
    }

    $artifact = Get-Item -LiteralPath $zipPath
    Write-Host "Portable bundle created: $($artifact.FullName) ($($artifact.Length) bytes)"
}
finally {
    Pop-Location
}
