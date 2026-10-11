[CmdletBinding()]
param(
    [switch]$Check
)

$ErrorActionPreference = 'Stop'
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$noticePath = Join-Path $repoRoot 'THIRD-PARTY-NOTICES.md'
$targets = @(
    'x86_64-pc-windows-msvc',
    'x86_64-unknown-linux-gnu'
)
$overrides = @{
    'keyed_priority_queue@0.4.2' = @(
        'third-party\overrides\keyed_priority_queue-0.4.2-LICENSE-MIT.txt'
    )
    'mysql-common-derive@0.32.2' = @(
        'third-party\overrides\mysql-common-derive-0.32.2-LICENSE-APACHE.txt',
        'third-party\overrides\mysql-common-derive-0.32.2-LICENSE-MIT.txt'
    )
}
$licensePattern = '^(LICENSE|LICENCE|COPYING|NOTICE|UNLICENSE|COPYRIGHT|AUTHORS)(\..*|[-_ ].*)?$'

$metadataOutput = & cargo metadata --format-version 1 --locked
if ($LASTEXITCODE -ne 0) {
    throw 'cargo metadata failed while collecting dependency licenses.'
}
$metadata = ($metadataOutput -join [Environment]::NewLine) | ConvertFrom-Json
$packagesById = @{}
foreach ($package in $metadata.packages) {
    $packagesById["$($package.name)@$($package.version)"] = $package
}

$runtimePackageIds = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
foreach ($target in $targets) {
    $treeOutput = & cargo tree --locked --target $target --edges normal --prefix none --color never
    if ($LASTEXITCODE -ne 0) {
        throw "cargo tree failed for target '$target'."
    }

    foreach ($line in $treeOutput) {
        if ($line -match '^\s*([A-Za-z0-9_-]+)\s+v([^\s]+)') {
            $packageId = "$($Matches[1])@$($Matches[2])"
            if ($packageId -ne 'exile-extdb3@0.1.0') {
                [void]$runtimePackageIds.Add($packageId)
            }
        }
    }
}

$dependencyRows = [System.Collections.Generic.List[object]]::new()
$licenseGroups = [System.Collections.Generic.Dictionary[string, object]]::new([System.StringComparer]::Ordinal)
$sha256 = [System.Security.Cryptography.SHA256]::Create()

foreach ($packageId in ($runtimePackageIds | Sort-Object)) {
    $package = $packagesById[$packageId]
    if ($null -eq $package) {
        throw "cargo metadata has no package record for '$packageId'."
    }
    if ([string]::IsNullOrWhiteSpace($package.license)) {
        throw "Dependency '$packageId' has no declared license expression."
    }

    $packageDirectory = Split-Path -Parent $package.manifest_path
    $candidatePaths = [System.Collections.Generic.List[string]]::new()
    if (-not [string]::IsNullOrWhiteSpace($package.license_file)) {
        $licenseFilePath = $package.license_file
        if (-not [System.IO.Path]::IsPathRooted($licenseFilePath)) {
            $licenseFilePath = Join-Path $packageDirectory $licenseFilePath
        }
        if (Test-Path -LiteralPath $licenseFilePath -PathType Leaf) {
            $candidatePaths.Add([System.IO.Path]::GetFullPath($licenseFilePath))
        }
    }

    if ($candidatePaths.Count -eq 0) {
        foreach ($licenseFile in (Get-ChildItem -LiteralPath $packageDirectory -File -Force | Where-Object { $_.Name -match $licensePattern } | Sort-Object Name)) {
            $candidatePaths.Add($licenseFile.FullName)
        }
    }

    if ($candidatePaths.Count -eq 0 -and $overrides.ContainsKey($packageId)) {
        foreach ($relativePath in $overrides[$packageId]) {
            $overridePath = Join-Path $repoRoot $relativePath
            if (-not (Test-Path -LiteralPath $overridePath -PathType Leaf)) {
                throw "Dependency license override is missing: $relativePath"
            }
            $candidatePaths.Add($overridePath)
        }
    }

    if ($candidatePaths.Count -eq 0) {
        throw "No license text or notice file was found for '$packageId'."
    }

    $dependencyRows.Add([pscustomobject]@{
        Name = $package.name
        Version = $package.version
        License = $package.license
    })

    foreach ($path in ($candidatePaths | Sort-Object -Unique)) {
        $text = [System.IO.File]::ReadAllText($path, [System.Text.Encoding]::UTF8)
        $text = $text.Replace([string]([char]13) + [char]10, [string][char]10).Replace([char]13, [char]10).Trim()
        if ([string]::IsNullOrWhiteSpace($text)) {
            throw "License file is empty: $path"
        }

        $textBytes = [System.Text.Encoding]::UTF8.GetBytes($text)
        $contentHash = [Convert]::ToHexString($sha256.ComputeHash($textBytes)).ToLowerInvariant()
        if (-not $licenseGroups.ContainsKey($contentHash)) {
            $licenseGroups[$contentHash] = [pscustomobject]@{
                Text = $text
                Users = [System.Collections.Generic.List[string]]::new()
            }
        }
        $licenseGroups[$contentHash].Users.Add("$packageId ($([System.IO.Path]::GetFileName($path)))")
    }
}

$outputLines = [System.Collections.Generic.List[string]]::new()
$outputLines.Add('# Third-party notices')
$outputLines.Add('')
$outputLines.Add('This file lists and reproduces the license and notice files for the normal dependency graphs used by the Windows and Linux release targets. The list is derived from Cargo.lock and the package metadata for each target.')
$outputLines.Add('')
$outputLines.Add('Where a published crate omits its license files, the pinned upstream copies are kept under third-party/overrides and correspond to the source commit recorded in that crate package metadata.')
$outputLines.Add('')
$outputLines.Add('Pinned upstream copies:')
$outputLines.Add('- keyed_priority_queue 0.4.2: https://github.com/AngelicosPhosphoros/keyed_priority_queue/blob/2656a970a1ff174010bb8ebfb676f922d2d59599/LICENSE.md')
$outputLines.Add('- mysql-common-derive 0.32.2: https://github.com/blackbeam/rust_mysql_common/blob/5d595826c93fd614b713cb7888a2055206ce40e4/LICENSE-APACHE and LICENSE-MIT')
$outputLines.Add('')
$outputLines.Add('The project itself is licensed under LGPL-3.0-only; see LICENSE. These entries apply to third-party dependencies and retain their respective license terms.')
$outputLines.Add('')
$outputLines.Add('## Dependency licenses')
$outputLines.Add('')
$outputLines.Add('| Package | Version | Declared license |')
$outputLines.Add('| --- | --- | --- |')
foreach ($row in ($dependencyRows | Sort-Object Name, Version)) {
    $outputLines.Add("| $($row.Name) | $($row.Version) | $($row.License) |")
}
$outputLines.Add('')
$outputLines.Add('## License and notice texts')
$outputLines.Add('')
$outputLines.Add('The following sections contain the license or notice text distributed by each dependency. Identical text is included once and lists every package that uses it.')
$outputLines.Add('')

foreach ($contentHash in ($licenseGroups.Keys | Sort-Object)) {
    $group = $licenseGroups[$contentHash]
    $users = @($group.Users | Sort-Object -Unique)
    $outputLines.Add("### Applies to $($users -join ', ')")
    $outputLines.Add('')
    $outputLines.Add('~~~~text')
    foreach ($line in ($group.Text -split [char]10)) {
        $outputLines.Add($line)
    }
    $outputLines.Add('~~~~')
    $outputLines.Add('')
}

$generated = [string]::Join([string][char]10, $outputLines) + [char]10
if ($Check) {
    if (-not (Test-Path -LiteralPath $noticePath -PathType Leaf)) {
        throw 'THIRD-PARTY-NOTICES.md is missing. Run this script without -Check to generate it.'
    }
    $existing = [System.IO.File]::ReadAllText($noticePath, [System.Text.Encoding]::UTF8)
    $existing = $existing.Replace([string]([char]13) + [char]10, [string][char]10).Replace([char]13, [char]10)
    if ($existing -cne $generated) {
        throw 'THIRD-PARTY-NOTICES.md is out of date. Run this script without -Check and review the result.'
    }
    Write-Host "Third-party notices are current for $($dependencyRows.Count) runtime packages."
}
else {
    [System.IO.File]::WriteAllText($noticePath, $generated, [System.Text.UTF8Encoding]::new($false))
    Write-Host "Generated THIRD-PARTY-NOTICES.md for $($dependencyRows.Count) runtime packages."
}

$sha256.Dispose()
