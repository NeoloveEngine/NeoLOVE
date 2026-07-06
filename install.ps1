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
    Install-WingetPackage "Git.Git" @("--scope", "user", "--force")
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
    Install-WingetPackage "Microsoft.VisualStudio.2022.BuildTools" @("--override", $override, "--force")

    if (-not (Test-VisualCppBuildTools)) {
        throw "The Visual Studio C++ build workload did not install successfully."
    }
}

function Install-Rust {
    Refresh-ProcessPath
    if ((Get-Command cargo -ErrorAction SilentlyContinue) -and (Get-Command rustc -ErrorAction SilentlyContinue)) {
        Write-Step "Rust toolchain already installed: $(& rustc --version)"
        return
    }

    if (Get-Command rustup -ErrorAction SilentlyContinue) {
        Write-Step "Completing the existing Rust toolchain installation"
    } else {
        Assert-Winget
        Write-Step "Installing the stable Rust toolchain"
        Install-WingetPackage "Rustlang.Rustup" @("--force")
        Refresh-ProcessPath
    }

    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
        throw "rustup installed, but was not found. Open a new PowerShell window and re-run this script."
    }

    & rustup toolchain install stable --profile minimal
    if ($LASTEXITCODE -ne 0) {
        throw "rustup could not install the stable Rust toolchain. Re-run this script to resume setup."
    }

    & rustup default stable
    if ($LASTEXITCODE -ne 0) {
        throw "rustup could not select the stable Rust toolchain. Re-run this script to resume setup."
    }

    Refresh-ProcessPath
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue) -or -not (Get-Command rustc -ErrorAction SilentlyContinue)) {
        throw "The Rust toolchain installation did not complete successfully. Re-run this script to resume setup."
    }
}

function Test-RepositoryRemoteMatches([string]$Path) {
    try {
        $remoteOutput = & git -C $Path remote get-url origin 2>$null
        if ($LASTEXITCODE -ne 0) {
            return $false
        }
    } catch {
        return $false
    }

    $remote = ($remoteOutput | Select-Object -First 1) -as [string]
    if ([string]::IsNullOrWhiteSpace($remote)) {
        return $false
    }
    $remote = $remote.Trim()
    $repositoryUrlWithoutSuffix = $RepositoryUrl.Substring(0, $RepositoryUrl.Length - 4)
    return ($remote -eq $RepositoryUrl -or $remote -eq $repositoryUrlWithoutSuffix)
}

function Test-RepositoryComplete([string]$Path) {
    if (-not (Test-RepositoryRemoteMatches $Path)) {
        return $false
    }

    try {
        & git -C $Path rev-parse --verify HEAD *> $null
        if ($LASTEXITCODE -ne 0) {
            return $false
        }
    } catch {
        return $false
    }

    return (Test-Path (Join-Path $Path "Cargo.toml") -PathType Leaf)
}

function Remove-StaleStagingRepository([string]$StagingDirectory) {
    if (-not (Test-Path $StagingDirectory)) {
        return
    }

    $marker = Join-Path $StagingDirectory ".neolove-installer"
    $markerContents = @(
        if (Test-Path $marker -PathType Leaf) {
            Get-Content $marker
        }
    )
    if ($markerContents.Count -eq 0 -or $markerContents[0] -ne $RepositoryUrl) {
        throw "$StagingDirectory already exists and was not created by the NeoLOVE installer. Move it elsewhere and re-run this script."
    }

    $ownerProcessId = 0
    if ($markerContents.Count -ge 2 -and
        [int]::TryParse($markerContents[1], [ref]$ownerProcessId) -and
        $ownerProcessId -ne $PID -and
        (Get-Process -Id $ownerProcessId -ErrorAction SilentlyContinue)) {
        throw "Another NeoLOVE installer is currently using $StagingDirectory (process $ownerProcessId)."
    }

    Write-Step "Cleaning up an interrupted NeoLOVE clone"
    Remove-Item -Recurse -Force $StagingDirectory
}

function Copy-RepositoryTransactionally {
    $stagingDirectory = "$InstallDirectory.installing"
    Remove-StaleStagingRepository $stagingDirectory

    New-Item -ItemType Directory -Path $stagingDirectory | Out-Null
    $marker = Join-Path $stagingDirectory ".neolove-installer"
    Set-Content -Path $marker -Value @($RepositoryUrl, $PID)
    $checkout = Join-Path $stagingDirectory "checkout"

    Write-Step "Cloning NeoLOVE into $InstallDirectory"
    & git clone $RepositoryUrl $checkout
    if ($LASTEXITCODE -ne 0) {
        throw "Git could not clone $RepositoryUrl. Re-run this script to clean up and retry the interrupted clone."
    }

    if (Test-Path $InstallDirectory) {
        throw "$InstallDirectory appeared while NeoLOVE was being cloned. The completed clone remains in $checkout."
    }

    Move-Item -Path $checkout -Destination $InstallDirectory
    Remove-Item $marker
    Remove-Item $stagingDirectory
}

function Copy-NeoLoveRepository {
    Remove-StaleStagingRepository "$InstallDirectory.installing"
    $gitDirectory = Join-Path $InstallDirectory ".git"
    if (Test-RepositoryComplete $InstallDirectory) {
        Write-Step "Existing NeoLOVE installation found"
        $changes = & git -C $InstallDirectory status --porcelain
        if ($LASTEXITCODE -ne 0) {
            throw "The existing NeoLOVE clone could not be inspected. Repair it or move it elsewhere, then re-run this script."
        }

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

    if (Test-Path $gitDirectory) {
        if (-not (Test-RepositoryRemoteMatches $InstallDirectory)) {
            throw "$InstallDirectory exists but is not a clone of $RepositoryUrl"
        }

        $timestamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
        $backup = "$InstallDirectory.incomplete-$timestamp-$PID"
        Write-Step "Preserving an incomplete NeoLOVE checkout at $backup"
        Move-Item -Path $InstallDirectory -Destination $backup
        Copy-RepositoryTransactionally
        return
    }

    if (Test-Path $InstallDirectory) {
        $items = @(Get-ChildItem -Force $InstallDirectory)
        if ($items.Count -eq 0) {
            Remove-Item $InstallDirectory
            Copy-RepositoryTransactionally
            return
        }

        throw "$InstallDirectory already exists and is not a Git repository. Move it elsewhere and re-run this script."
    }

    New-Item -ItemType Directory -Force -Path (Split-Path $InstallDirectory) | Out-Null
    Copy-RepositoryTransactionally
}

function Test-VulkanRuntime {
    $vulkanInfo = Get-Command vulkaninfo.exe -ErrorAction SilentlyContinue
    if ($vulkanInfo) {
        try {
            & $vulkanInfo.Source --summary *> $null
            return ($LASTEXITCODE -eq 0)
        } catch {
            # Windows PowerShell promotes native stderr to NativeCommandError when
            # ErrorActionPreference is Stop. Driver/registry errors mean that the
            # Vulkan runtime is unusable, but must not abort the installer.
            return $false
        }
    }

    # GPU drivers install the Vulkan loader here when the machine supports it.
    return (Test-Path (Join-Path $env:WINDIR "System32\vulkan-1.dll"))
}

Refresh-ProcessPath
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
