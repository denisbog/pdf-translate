#requires -Version 5.1
<#
.SYNOPSIS
    Downloads the ONNX Runtime CUDA build and the CUDA / cuBLAS / cuFFT /
    cuDNN 9 DLLs that it loads, into the repository's `ort\` folder.

.DESCRIPTION
    PDF Translate runs Qwen2.5 through ONNX Runtime's CUDA execution provider.
    Windows already has an `onnxruntime.dll` in `C:\Windows\System32` (Windows
    ML / WebView2), but it is far too old for the models: it reports "max
    supported IR version: 9" while the Qwen2.5 exports are IR 10. Nothing is
    actually wrong with that copy — it just must not be the one the process
    loads, so this script puts a current runtime where the application looks
    first (`ort\`).

    `onnxruntime_providers_cuda.dll` imports the CUDA runtime, cuBLAS, cuFFT
    and cuDNN 9 by name, so those DLLs have to sit next to `onnxruntime.dll`;
    when one is missing the provider cannot load and the session would fall
    back to the CPU (the application now reports that as an error naming the
    missing DLL).

    Re-running the script is cheap: archives are cached in `ort\.downloads`
    and everything that is already in place is skipped. Pass -Force to
    re-download.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tools\fetch-ort-windows.ps1

.EXAMPLE
    pwsh -File tools\fetch-ort-windows.ps1 -OrtVersion 1.23.0
#>
[CmdletBinding()]
param(
    # Repository root; defaults to the parent of this script's folder.
    [string]$Root = '',
    # ONNX Runtime release to fetch. Must be >= 1.18 (model IR 10) and built
    # against CUDA 12.x / cuDNN 9.
    [string]$OrtVersion = '1.22.0',
    # NVIDIA runtime wheels (PyPI). Must stay on CUDA 12.x and cuDNN 9.x.
    [string]$CudaVersion = '12.9.79',
    [string]$CublasVersion = '12.9.2.10',
    [string]$CufftVersion = '11.4.1.4',
    [string]$CudnnVersion = '9.9.0.52',
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# `$PSScriptRoot` is not reliable in a parameter default under Windows
# PowerShell, so resolve the repository root here.
if (-not $Root) {
    $scriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
    $Root = Split-Path -Parent $scriptDir
}
if (-not (Test-Path -LiteralPath $Root)) {
    New-Item -ItemType Directory -Force -Path $Root | Out-Null
}
$Root = (Resolve-Path -Path $Root).Path

$name = "onnxruntime-win-x64-gpu-$OrtVersion"
$ortDir = Join-Path (Join-Path $Root 'ort') $name
$libDir = Join-Path $ortDir 'lib'
$cache = Join-Path (Join-Path $Root 'ort') '.downloads'

New-Item -ItemType Directory -Force -Path $libDir, $cache | Out-Null

function Save-Download {
    param([string]$Url, [string]$Path)

    if ((Test-Path $Path) -and -not $Force) {
        Write-Host "cached    $(Split-Path -Leaf $Path)"
        return
    }

    Write-Host "download  $Url"
    if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
        & curl.exe --fail --location --silent --show-error --output $Path $Url
        if ($LASTEXITCODE -ne 0) { throw "download failed: $Url" }
    } else {
        Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $Path
    }
}

# The wheel URL on PyPI contains a content hash, so it cannot be constructed
# from the package and version alone: ask the JSON API for it.
function Get-PyPiWheel {
    param([string]$Package, [string]$Version)

    $meta = Invoke-RestMethod -Uri "https://pypi.org/pypi/$Package/$Version/json"
    $file = $meta.urls | Where-Object { $_.filename -like '*-win_amd64.whl' } | Select-Object -First 1
    if (-not $file) { throw "no Windows wheel found for $Package $Version" }
    return $file
}

# A wheel or zip is expanded with .NET directly: `Expand-Archive` insists on a
# `.zip` extension, and copying a 700 MB wheel to rename it would double the
# cache for nothing.
function Expand-To {
    param([string]$Archive, [string]$Destination)

    if (Test-Path $Destination) { Remove-Item -Recurse -Force $Destination }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::ExtractToDirectory($Archive, $Destination)
}

# 1. ONNX Runtime itself (onnxruntime.dll + onnxruntime_providers_*.dll).
if ((Test-Path (Join-Path $libDir 'onnxruntime.dll')) -and -not $Force) {
    Write-Host "cached    $name"
} else {
    $zipPath = Join-Path $cache "$name.zip"
    $url = "https://github.com/microsoft/onnxruntime/releases/download/v$OrtVersion/$name.zip"
    Save-Download -Url $url -Path $zipPath

    Write-Host "extract   $name.zip"
    $staging = Join-Path $cache "$name-extract"
    Expand-To -Archive $zipPath -Destination $staging

    # The archive holds <name>\lib\*.dll (and .pdb/.lib we do not need).
    $from = Join-Path (Join-Path $staging $name) 'lib'
    if (-not (Test-Path $from)) { $from = Join-Path $staging 'lib' }
    if (-not (Test-Path (Join-Path $from 'onnxruntime.dll'))) {
        throw "onnxruntime.dll not found in $zipPath"
    }
    try {
        Get-ChildItem -Path $from -Filter '*.dll' -File | Copy-Item -Destination $libDir -Force -ErrorAction Stop
    } catch {
        throw "cannot write to $libDir - close any running pdf-translate.exe and re-run. $($_.Exception.Message)"
    }
    Remove-Item -Recurse -Force $staging
}

# 2. CUDA runtime, cuBLAS, cuFFT and cuDNN 9, next to onnxruntime.dll.
$wheels = @(
    @{ Package = 'nvidia-cuda-runtime-cu12'; Version = $CudaVersion;   Dlls = @('cudart64_12.dll') },
    @{ Package = 'nvidia-cublas-cu12';       Version = $CublasVersion; Dlls = @('cublas64_12.dll', 'cublasLt64_12.dll') },
    @{ Package = 'nvidia-cufft-cu12';        Version = $CufftVersion;  Dlls = @('cufft64_11.dll') },
    @{ Package = 'nvidia-cudnn-cu12';        Version = $CudnnVersion;  Dlls = @('cudnn*_9.dll') }
)

foreach ($wheel in $wheels) {
    $file = Get-PyPiWheel -Package $wheel.Package -Version $wheel.Version
    $missing = $wheel.Dlls | Where-Object {
        -not (Get-ChildItem -Path $libDir -Filter $_ -File -ErrorAction SilentlyContinue)
    }
    if (-not $missing -and -not $Force) {
        Write-Host "cached    $($wheel.Package) $($wheel.Version)"
        continue
    }

    $wheelPath = Join-Path $cache $file.filename
    Save-Download -Url $file.url -Path $wheelPath

    $unpacked = Join-Path $cache ([IO.Path]::GetFileNameWithoutExtension($file.filename))
    Expand-To -Archive $wheelPath -Destination $unpacked
    foreach ($pattern in $wheel.Dlls) {
        $found = Get-ChildItem -Path $unpacked -Recurse -Filter $pattern -File
        if (-not $found) { throw "$($wheel.Package) $($wheel.Version) has no $pattern" }
        $found | Copy-Item -Destination $libDir -Force
    }
    Remove-Item -Recurse -Force $unpacked
}

# 3. Summary.
$dlls = Get-ChildItem -Path $libDir -Filter '*.dll' -File | Sort-Object Name
Write-Host ''
Write-Host "ONNX Runtime $OrtVersion with CUDA / cuDNN in $libDir"
foreach ($dll in $dlls) {
    Write-Host ('  {0,-48} {1,8:N1} MB' -f $dll.Name, ($dll.Length / 1MB))
}
Write-Host ''
Write-Host 'Next: cargo run --release -- "C:\path\to\document.pdf"'
Write-Host 'The log (and --translate) should report this onnxruntime.dll and "on cuda (int4)".'
