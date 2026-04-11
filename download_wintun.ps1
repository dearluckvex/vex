# WinTun 自动下载脚本
# 运行方式: powershell -ExecutionPolicy Bypass -File download_wintun.ps1

$ErrorActionPreference = "Stop"

Write-Host "正在下载 WinTun x64 版本..." -ForegroundColor Green

# WinTun 下载 URL（最新版本）
$WinTunUrl = "https://www.wintun.net/builds/wintun-0.14.1.zip"
$ZipPath = "$PSScriptRoot\wintun-0.14.1.zip"
$ExtractPath = "$PSScriptRoot\wintun-extract"

try {
    # 下载
    Write-Host "下载中..." -ForegroundColor Yellow
    Invoke-WebRequest -Uri $WinTunUrl -OutFile $ZipPath
    Write-Host "✓ 下载完成" -ForegroundColor Green
    
    # 解压
    Write-Host "解压中..." -ForegroundColor Yellow
    Expand-Archive -Path $ZipPath -DestinationPath $ExtractPath -Force
    Write-Host "✓ 解压完成" -ForegroundColor Green
    
    # 复制 x64 DLL
    Write-Host "复制 wintun.dll..." -ForegroundColor Yellow
    $DllSource = "$ExtractPath\wintun\bin\amd64\wintun.dll"
    $DllDest = "$PSScriptRoot\wintun.dll"
    
    if (Test-Path $DllSource) {
        Copy-Item -Path $DllSource -Destination $DllDest -Force
        Write-Host "✓ wintun.dll 已复制到项目根目录" -ForegroundColor Green
        Write-Host "  位置: $DllDest" -ForegroundColor Cyan
    } else {
        Write-Host "✗ 找不到 $DllSource" -ForegroundColor Red
        exit 1
    }
    
    # 清理临时文件
    Write-Host "清理临时文件..." -ForegroundColor Yellow
    Remove-Item -Path $ZipPath -Force
    Remove-Item -Path $ExtractPath -Recurse -Force
    
    Write-Host ""
    Write-Host "╔════════════════════════════════════════════════════════════════╗" -ForegroundColor Green
    Write-Host "║               ✓ WinTun 下载和安装完成！                        ║" -ForegroundColor Green
    Write-Host "╚════════════════════════════════════════════════════════════════╝" -ForegroundColor Green
    Write-Host ""
    Write-Host "接下来请运行:" -ForegroundColor Yellow
    Write-Host "  cargo build --release" -ForegroundColor Cyan
    Write-Host "  .\target\release\xtune.exe" -ForegroundColor Cyan
    
} catch {
    Write-Host "✗ 错误: $_" -ForegroundColor Red
    exit 1
}
