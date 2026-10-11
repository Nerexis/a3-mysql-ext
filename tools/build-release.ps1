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
$releaseRoot = Join-Path $distRoot 'release'
New-Item -ItemType Directory -Force -Path $distRoot, $releaseRoot | Out-Null

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

$rustVersionOutput = & rustc --version
if ($LASTEXITCODE -ne 0) {
    throw 'rustc --version failed.'
}
$rustVersion = $rustVersionOutput.Trim()
if ($rustVersion -notmatch '^rustc 1\.99\.0(?:\s|$)') {
    throw "Release toolchain must be Rust 1.99.0; found '$rustVersion'."
}

function Invoke-WindowsCargo {
    param([Parameter(Mandatory)][string[]]$CommandArgs)

    Push-Location $repoRoot
    try {
        & cargo @CommandArgs
        if ($LASTEXITCODE -ne 0) {
            throw "cargo $($CommandArgs -join ' ') failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }
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

$linuxRustVersionOutput = & wsl.exe --distribution $LinuxDistribution --exec $LinuxRustc --version
if ($LASTEXITCODE -ne 0) {
    throw "Could not read the Linux Rust toolchain version from '$LinuxDistribution'."
}
$linuxRustVersion = $linuxRustVersionOutput.Trim()
if ($linuxRustVersion -notmatch '^rustc 1\.99\.0(?:\s|$)') {
    throw "Release toolchain must be Rust 1.99.0 on Linux; found '$linuxRustVersion'."
}

function Invoke-LinuxCargo {
    param([Parameter(Mandatory)][string[]]$CommandArgs)

    $wslArgs = @(
        '--distribution', $LinuxDistribution,
        '--cd', $linuxRepo,
        '--exec', 'env', 'CARGO_TARGET_DIR=/tmp/extdb3-linux-target',
        $LinuxCargo
    ) + $CommandArgs
    & wsl.exe @wslArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Linux cargo $($CommandArgs -join ' ') failed with exit code $LASTEXITCODE."
    }
}

Invoke-WindowsCargo @('fmt', '--all', '--', '--check')
Invoke-WindowsCargo @('test', '--locked', '--target', 'x86_64-pc-windows-msvc')
Invoke-WindowsCargo @('clippy', '--locked', '--all-targets', '--target', 'x86_64-pc-windows-msvc', '--', '-D', 'warnings')
Invoke-WindowsCargo @('build', '--locked', '--release', '--target', 'x86_64-pc-windows-msvc')
Invoke-WindowsCargo @('bench', '--locked', '--bench', 'synthetic', '--features', 'synthetic-bench', '--no-run', '--target', 'x86_64-pc-windows-msvc')

Invoke-LinuxCargo @('fmt', '--all', '--', '--check')
Invoke-LinuxCargo @('test', '--locked', '--target', 'x86_64-unknown-linux-gnu')
Invoke-LinuxCargo @('clippy', '--locked', '--all-targets', '--target', 'x86_64-unknown-linux-gnu', '--', '-D', 'warnings')
Invoke-LinuxCargo @('build', '--locked', '--release', '--target', 'x86_64-unknown-linux-gnu')
Invoke-LinuxCargo @('bench', '--locked', '--bench', 'synthetic', '--features', 'synthetic-bench', '--no-run', '--target', 'x86_64-unknown-linux-gnu')

& (Join-Path $repoRoot 'tools\generate-third-party-notices.ps1') -Check

$windowsArtifact = Join-Path $repoRoot 'target\x86_64-pc-windows-msvc\release\extDB3_x64.dll'
if (-not (Test-Path -LiteralPath $windowsArtifact -PathType Leaf)) {
    throw "Windows release artifact is missing: $windowsArtifact"
}
Copy-Item -LiteralPath $windowsArtifact -Destination (Join-Path $distRoot 'extDB3_x64.dll') -Force

$linuxArtifact = '/tmp/extdb3-linux-target/x86_64-unknown-linux-gnu/release/libextDB3_x64.so'
$linuxDistArtifact = "$linuxRepo/dist/extDB3_x64.so"
$copyLinuxArgs = @('--distribution', $LinuxDistribution, '--cd', $linuxRepo, '--exec', 'cp', $linuxArtifact, $linuxDistArtifact)
& wsl.exe @copyLinuxArgs
if ($LASTEXITCODE -ne 0) {
    throw 'Could not copy the Linux release artifact into dist.'
}

$windowsDistArtifact = Join-Path $distRoot 'extDB3_x64.dll'
$linuxHostArtifact = Join-Path $distRoot 'extDB3_x64.so'
if (-not (Test-Path -LiteralPath $linuxHostArtifact -PathType Leaf)) {
    throw "Linux release artifact is missing: $linuxHostArtifact"
}

$windowsAbiReport = Join-Path $env:TEMP 'extdb3-release-windows-abi.json'
& python (Join-Path $repoRoot 'tools\abi_probe.py') $windowsDistArtifact --output $windowsAbiReport | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw 'Windows release ABI probe failed.'
}

$linuxAbiReport = '/tmp/extdb3-release-linux-abi.json'
$linuxAbiArgs = @(
    '--distribution', $LinuxDistribution,
    '--cd', $linuxRepo,
    '--exec', 'python3', 'tools/abi_probe.py', $linuxDistArtifact,
    '--output', $linuxAbiReport
)
& wsl.exe @linuxAbiArgs | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw 'Linux release ABI probe failed.'
}

$packageName = "exile-extdb3-v$ReleaseVersion-win64-linux-x64"
$archivePath = Join-Path $releaseRoot "$packageName.zip"
$stagePath = Join-Path $releaseRoot ('.stage-' + [Guid]::NewGuid().ToString('N'))
$windowsStage = Join-Path $stagePath 'windows-x64'
$linuxStage = Join-Path $stagePath 'linux-x64'
New-Item -ItemType Directory -Force -Path $windowsStage, $linuxStage | Out-Null

Copy-Item -LiteralPath $windowsDistArtifact -Destination (Join-Path $windowsStage 'extDB3_x64.dll')
Copy-Item -LiteralPath $linuxHostArtifact -Destination (Join-Path $linuxStage 'extDB3_x64.so')
Copy-Item -LiteralPath (Join-Path $repoRoot 'release\README.md') -Destination (Join-Path $stagePath 'README.md')
Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE') -Destination (Join-Path $stagePath 'LICENSE')
Copy-Item -LiteralPath (Join-Path $repoRoot 'THIRD-PARTY-NOTICES.md') -Destination (Join-Path $stagePath 'THIRD-PARTY-NOTICES.md')

$artifactManifest = @(
    [ordered]@{
        path = 'windows-x64/extDB3_x64.dll'
        sha256 = (Get-FileHash -LiteralPath (Join-Path $windowsStage 'extDB3_x64.dll') -Algorithm SHA256).Hash.ToLowerInvariant()
    },
    [ordered]@{
        path = 'linux-x64/extDB3_x64.so'
        sha256 = (Get-FileHash -LiteralPath (Join-Path $linuxStage 'extDB3_x64.so') -Algorithm SHA256).Hash.ToLowerInvariant()
    }
)
$manifest = [ordered]@{
    release = $ReleaseVersion
    license = 'LGPL-3.0-only'
    sourceCommit = $sourceCommit
    protocolVersion = '1.033'
    compatibilityTarget = 'extDB3 v1.033.1'
    differentialReference = 'extDB3 v1.031'
    rustc = $rustVersion
    linuxRustc = $linuxRustVersion
    builtAtUtc = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    targets = @('x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu')
    artifacts = $artifactManifest
}
$manifestJson = $manifest | ConvertTo-Json -Depth 6
[System.IO.File]::WriteAllText(
    (Join-Path $stagePath 'manifest.json'),
    $manifestJson + [Environment]::NewLine,
    [System.Text.UTF8Encoding]::new($false)
)

Compress-Archive -Path (Join-Path $stagePath '*') -DestinationPath $archivePath -CompressionLevel Optimal -Force
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead($archivePath)
try {
    $entryNames = @($zip.Entries | ForEach-Object FullName)
    foreach ($requiredEntry in @('README.md', 'LICENSE', 'THIRD-PARTY-NOTICES.md', 'manifest.json', 'windows-x64/extDB3_x64.dll', 'linux-x64/extDB3_x64.so')) {
        if ($entryNames -notcontains $requiredEntry) {
            throw "Release archive is missing '$requiredEntry'."
        }
    }
}
finally {
    $zip.Dispose()
}

$sourceArchivePath = Join-Path $releaseRoot "$packageName-source.zip"
$sourcePrefix = "$packageName-source/"
& git -C $repoRoot archive --format=zip "--prefix=$sourcePrefix" "--output=$sourceArchivePath" $sourceCommit
if ($LASTEXITCODE -ne 0) {
    throw 'Could not create the matching source archive.'
}

$sourceZip = [System.IO.Compression.ZipFile]::OpenRead($sourceArchivePath)
try {
    $sourceEntryNames = @($sourceZip.Entries | ForEach-Object FullName)
    foreach ($requiredEntry in @(
        ($sourcePrefix + 'README.md'),
        ($sourcePrefix + 'LICENSE'),
        ($sourcePrefix + 'THIRD-PARTY-NOTICES.md'),
        ($sourcePrefix + 'tools/generate-third-party-notices.ps1'),
        ($sourcePrefix + 'Cargo.toml'),
        ($sourcePrefix + 'Cargo.lock'),
        ($sourcePrefix + 'src/lib.rs')
    )) {
        if ($sourceEntryNames -notcontains $requiredEntry) {
            throw "Source archive is missing '$requiredEntry'."
        }
    }
}
finally {
    $sourceZip.Dispose()
}

$archiveHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
$checksumPath = "$archivePath.sha256"
[System.IO.File]::WriteAllText(
    $checksumPath,
    "$archiveHash *$([System.IO.Path]::GetFileName($archivePath))$([Environment]::NewLine)",
    [System.Text.Encoding]::ASCII
)

$sourceArchiveHash = (Get-FileHash -LiteralPath $sourceArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
$sourceChecksumPath = "$sourceArchivePath.sha256"
[System.IO.File]::WriteAllText(
    $sourceChecksumPath,
    "$sourceArchiveHash *$([System.IO.Path]::GetFileName($sourceArchivePath))$([Environment]::NewLine)",
    [System.Text.Encoding]::ASCII
)

$resolvedReleaseRoot = [System.IO.Path]::GetFullPath($releaseRoot)
$resolvedStagePath = [System.IO.Path]::GetFullPath($stagePath)
$releasePrefix = $resolvedReleaseRoot + [System.IO.Path]::DirectorySeparatorChar
if (-not $resolvedStagePath.StartsWith($releasePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to remove staging path outside release directory: $resolvedStagePath"
}
Remove-Item -LiteralPath $resolvedStagePath -Recurse -Force

Write-Host "Release archive: $archivePath"
Write-Host "SHA-256 file: $checksumPath"
Write-Host "Source archive: $sourceArchivePath"
Write-Host "Source SHA-256 file: $sourceChecksumPath"
Write-Host "Windows artifact: $windowsDistArtifact"
Write-Host "Linux artifact: $linuxHostArtifact"
