[CmdletBinding()]
param(
    [ValidateSet("Auto", "On", "Off")]
    [string]$Vulkan = "Auto"
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
Set-StrictMode -Version Latest

$RepositoryUrl = "https://github.com/NeoloveEngine/NeoLOVE.git"
$InstallDirectory = Join-Path $env:LOCALAPPDATA "NeoLOVE"

function Write-Step([string]$Message) {
    Write-Host "`n==> $Message" -ForegroundColor Cyan
}

function Refresh-ProcessPath {
    $machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $knownPaths = @(
        (Join-Path $HOME ".cargo\bin"),
        (Join-Path $env:LOCALAPPDATA "Programs\Git\cmd"),
        (Join-Path $env:ProgramFiles "Git\cmd")
    )
    $env:Path = (@($machinePath, $userPath) + $knownPaths | Where-Object { $_ }) -join ";"
}

function Assert-Winget {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        throw "Windows Package Manager (winget) is required. Install 'App Installer' from Microsoft, then re-run this script."
    }
}

function Install-WingetPackage([string]$Id, [string[]]$AdditionalArguments = @()) {
    $arguments = @(
        "install", "--id", $Id, "--exact", "--source", "winget", "--silent",
        "--accept-package-agreements", "--accept-source-agreements"
    ) + $AdditionalArguments
    & winget @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "winget could not install $Id (exit code $LASTEXITCODE)."
    }
}

function Install-Git {
    if (Get-Command git -ErrorAction SilentlyContinue) {
        Write-Step "Git already installed: $(& git --version)"
        return
    }

    Assert-Winget
    Write-Step "Installing Git"
    Install-WingetPackage "Git.Git" @("--scope", "user")
    Refresh-ProcessPath
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        throw "Git installed, but git.exe was not found. Open a new PowerShell window and re-run this script."
    }
}

function Test-VisualCppBuildTools {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) {
        return $false
    }

    $installation = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    return (-not [string]::IsNullOrWhiteSpace(($installation | Select-Object -First 1)))
}

function Install-VisualCppBuildTools {
    if (Test-VisualCppBuildTools) {
        Write-Step "Visual Studio C++ build tools already installed"
        return
    }

    Assert-Winget
    Write-Step "Installing Visual Studio 2022 Build Tools and the Desktop C++ workload"
    $override = "--wait --passive --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
    Install-WingetPackage "Microsoft.VisualStudio.2022.BuildTools" @("--override", $override)

    if (-not (Test-VisualCppBuildTools)) {
        throw "The Visual Studio C++ build workload did not install successfully."
    }
}

function Install-Rust {
    if ((Get-Command cargo -ErrorAction SilentlyContinue) -and (Get-Command rustc -ErrorAction SilentlyContinue)) {
        Write-Step "Rust toolchain already installed: $(& rustc --version)"
        return
    }

    Assert-Winget
    Write-Step "Installing the stable Rust toolchain"
    Install-WingetPackage "Rustlang.Rustup"
    Refresh-ProcessPath
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
        throw "rustup installed, but was not found. Open a new PowerShell window and re-run this script."
    }

    & rustup default stable
    if ($LASTEXITCODE -ne 0) {
        throw "rustup could not install the stable Rust toolchain."
    }
}

function Copy-NeoLoveRepository {
    $gitDirectory = Join-Path $InstallDirectory ".git"
    if (Test-Path $gitDirectory) {
        $remote = (& git -C $InstallDirectory remote get-url origin 2>$null).Trim()
        $repositoryUrlWithoutSuffix = $RepositoryUrl.Substring(0, $RepositoryUrl.Length - 4)
        if ($remote -ne $RepositoryUrl -and $remote -ne $repositoryUrlWithoutSuffix) {
            throw "$InstallDirectory exists but is not a clone of $RepositoryUrl"
        }

        $changes = & git -C $InstallDirectory status --porcelain
        if ([string]::IsNullOrWhiteSpace(($changes -join "`n"))) {
            Write-Step "Updating the existing NeoLOVE clone"
            & git -C $InstallDirectory pull --ff-only
            if ($LASTEXITCODE -ne 0) {
                throw "The existing NeoLOVE clone could not be updated with a fast-forward pull."
            }
        } else {
            Write-Step "Existing clone has local changes; leaving them untouched"
        }
        return
    }

    if (Test-Path $InstallDirectory) {
        throw "$InstallDirectory already exists and is not a Git repository."
    }

    Write-Step "Cloning NeoLOVE into $InstallDirectory"
    New-Item -ItemType Directory -Force -Path (Split-Path $InstallDirectory) | Out-Null
    & git clone $RepositoryUrl $InstallDirectory
    if ($LASTEXITCODE -ne 0) {
        throw "Git could not clone $RepositoryUrl"
    }
}

function Test-VulkanRuntime {
    $vulkanInfo = Get-Command vulkaninfo.exe -ErrorAction SilentlyContinue
    if ($vulkanInfo) {
        & $vulkanInfo.Source --summary *> $null
        return ($LASTEXITCODE -eq 0)
    }

    # GPU drivers install the Vulkan loader here when the machine supports it.
    return (Test-Path (Join-Path $env:WINDIR "System32\vulkan-1.dll"))
}

Install-Git
Copy-NeoLoveRepository
Install-VisualCppBuildTools
Install-Rust
Refresh-ProcessPath

$cargoArguments = @("run", "--release", "--locked")
$enableVulkan = switch ($Vulkan) {
    "On" { $true }
    "Off" { $false }
    default { Test-VulkanRuntime }
}

if ($enableVulkan) {
    Write-Step "Compatible Vulkan runtime detected or requested; enabling the Vulkan renderer"
    $cargoArguments += @("--features", "vulkan")
} else {
    Write-Step "No working Vulkan runtime detected; using the software renderer"
}

$cargoArguments += @("--", "editor")
Write-Step "Compiling and launching NeoLOVE in release mode"
Push-Location $InstallDirectory
try {
    & cargo @cargoArguments
    if ($LASTEXITCODE -ne 0) {
        throw "NeoLOVE failed to compile or run (exit code $LASTEXITCODE)."
    }
} finally {
    Pop-Location
}
