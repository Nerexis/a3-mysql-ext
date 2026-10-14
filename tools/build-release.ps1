[CmdletBinding()]
param(
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$ReleaseVersion = '0.1.0',
    [string]$LinuxCargo = '/root/.cargo/bin/cargo',
    [string]$LinuxRustc = '/root/.cargo/bin/rustc',
    [string]$LinuxDistribution = 'Ubuntu'
)

$ErrorActionPreference = 'Stop'
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$distRoot = Join-Path $repoRoot 'dist'
$nativeRoot = Join-Path $distRoot 'native'
$releaseRoot = Join-Path $distRoot 'release'
New-Item -ItemType Directory -Force -Path $nativeRoot, $releaseRoot | Out-Null

$workingTreeStatus = & git -C $repoRoot status --porcelain --untracked-files=normal
if ($LASTEXITCODE -ne 0) {
    throw 'Could not inspect the Git worktree before preparing the release.'
}
if (-not [string]::IsNullOrWhiteSpace(($workingTreeStatus -join [Environment]::NewLine))) {
    throw 'Release builds require a clean Git worktree so the source archive matches the binaries.'
}
$sourceCommit = (& git -C $repoRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($sourceCommit)) {
    throw 'Could not resolve the source commit for the release.'
}

$linuxRepoOutput = & wsl.exe --distribution $LinuxDistribution --exec wslpath -a $repoRoot
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($linuxRepoOutput)) {
    throw "Could not resolve the repository path inside WSL distribution '$LinuxDistribution'."
}
$linuxRepo = $linuxRepoOutput.Trim()

$linuxOsRelease = & wsl.exe --distribution $LinuxDistribution --exec cat /etc/os-release
if ($LASTEXITCODE -ne 0) {
    throw "Could not read the Linux distribution version from '$LinuxDistribution'."
}
$linuxVersionLine = $linuxOsRelease | Where-Object { $_ -match '^VERSION_ID=' } | Select-Object -First 1
if ([string]::IsNullOrWhiteSpace($linuxVersionLine) -or $linuxVersionLine.Trim() -ne 'VERSION_ID="24.04"') {
    throw "Release builds require Ubuntu 24.04; distribution '$LinuxDistribution' reports '$linuxVersionLine'."
}

function Invoke-WindowsNativeChecks {
    param([Parameter(Mandatory)][string[]]$CommandArgs)

    Push-Location $repoRoot
    try {
        & python @CommandArgs
        if ($LASTEXITCODE -ne 0) {
            throw "Python $($CommandArgs -join ' ') failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }
}

$windowsTarget = 'x86_64-pc-windows-msvc'
$linuxTarget = 'x86_64-unknown-linux-gnu'
$windowsOutput = Join-Path $nativeRoot $windowsTarget
$linuxOutput = "dist/native/$linuxTarget"
$checkNative = Join-Path $repoRoot 'tools\check_native.py'

Invoke-WindowsNativeChecks @(
    $checkNative,
    '--target', $windowsTarget,
    '--output-dir', $windowsOutput,
    '--source-commit', $sourceCommit
)

$linuxCheckArgs = @(
    '--distribution', $LinuxDistribution,
    '--cd', $linuxRepo,
    '--exec', 'env', 'CARGO_TARGET_DIR=/tmp/extdb3-linux-target',
    'python3', 'tools/check_native.py',
    '--target', $linuxTarget,
    '--output-dir', $linuxOutput,
    '--cargo', $LinuxCargo,
    '--rustc', $LinuxRustc,
    '--source-commit', $sourceCommit
)
& wsl.exe @linuxCheckArgs
if ($LASTEXITCODE -ne 0) {
    throw 'Linux native release checks failed.'
}

& (Join-Path $repoRoot 'tools\generate-third-party-notices.ps1') -Check
if ($LASTEXITCODE -ne 0) {
    throw 'Third-party notice validation failed.'
}

Copy-Item -LiteralPath (Join-Path $windowsOutput 'extDB3_x64.dll') -Destination (Join-Path $distRoot 'extDB3_x64.dll') -Force
Copy-Item -LiteralPath (Join-Path $nativeRoot "$linuxTarget\extDB3_x64.so") -Destination (Join-Path $distRoot 'extDB3_x64.so') -Force

Invoke-WindowsNativeChecks @(
    (Join-Path $repoRoot 'tools\package_release.py'),
    '--version', $ReleaseVersion,
    '--source-commit', $sourceCommit,
    '--artifact-root', $nativeRoot,
    '--output-dir', $releaseRoot
)
