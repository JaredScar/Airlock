# Installs the CUDA 12.8 toolkit pieces Whisper needs, then copies the runtime
# DLLs next to the app. The display driver is not touched.
param(
    [string]$InstallDir = ""
)

$ErrorActionPreference = "Stop"
$toolkit = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.8"
$cudart = Join-Path $toolkit "bin\cudart64_12.dll"

function Copy-Runtime([string]$Destination) {
    if (-not $Destination -or -not (Test-Path $Destination)) {
        return
    }
    $bin = Join-Path $toolkit "bin"
    if (-not (Test-Path $bin)) {
        return
    }
    Get-ChildItem $bin -Filter "*.dll" | Where-Object {
        $_.Name -like "cudart64_*" -or
        $_.Name -like "cublas64_*" -or
        $_.Name -like "cublasLt64_*" -or
        $_.Name -like "nvJitLink*" -or
        $_.Name -like "nvrtc64_*"
    } | ForEach-Object {
        Copy-Item $_.FullName (Join-Path $Destination $_.Name) -Force
    }
}

if (-not (Test-Path $cudart)) {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        $arg = "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`""
        if ($InstallDir) {
            $arg += " -InstallDir `"$InstallDir`""
        }
        $elevated = Start-Process -FilePath "powershell.exe" -ArgumentList $arg -Verb RunAs -Wait -PassThru
        exit $elevated.ExitCode
    }

    $dir = Join-Path $env:TEMP "airlock-cuda"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $exe = Join-Path $dir "cuda_12.8.2_windows_network.exe"
    if (-not (Test-Path $exe) -or (Get-Item $exe).Length -lt 1000000) {
        $url = "https://developer.download.nvidia.com/compute/cuda/12.8.2/network_installers/cuda_12.8.2_windows_network.exe"
        & curl.exe -L --fail --retry 3 -o $exe $url
        if ($LASTEXITCODE -ne 0) {
            exit $LASTEXITCODE
        }
    }
    $install = Start-Process -FilePath $exe -ArgumentList @(
        "-s", "-n",
        "nvcc_12.8",
        "cudart_12.8",
        "cublas_12.8",
        "cublas_dev_12.8",
        "nvrtc_12.8",
        "nvjitlink_12.8",
        "thrust_12.8",
        "visual_studio_integration_12.8"
    ) -Wait -PassThru
    if ($install.ExitCode -ne 0) {
        exit $install.ExitCode
    }
}

$toolkitRoot = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.8"
if (Test-Path (Join-Path $toolkitRoot "bin\cudart64_12.dll")) {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        $key = "HKLM:\SOFTWARE\NVIDIA Corporation\GPU Computing Toolkit\Cuda\v12.8"
        New-Item -Path $key -Force | Out-Null
        Set-ItemProperty -Path $key -Name InstallDir -Value ($toolkitRoot + "\")
        [Environment]::SetEnvironmentVariable("CUDA_PATH", $toolkitRoot, "Machine")
        [Environment]::SetEnvironmentVariable("CUDA_PATH_V12_8", $toolkitRoot, "Machine")
    }
}

Copy-Runtime $InstallDir
exit 0
