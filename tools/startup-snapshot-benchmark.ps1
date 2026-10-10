[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })][string]$RunnerScript,
    [Parameter(Mandatory)][ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })][string]$MySqlClient,
    [Parameter(Mandatory)][ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })][string]$DefaultsExtraFile,
    [Parameter(Mandatory)][ValidateLength(1, 64)][ValidatePattern('^[A-Za-z0-9_]+$')][string]$Database,
    [Parameter(Mandatory)][ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })][string]$SnapshotSql,
    [Parameter(Mandatory)][ValidateRange(1, 100)][int]$RunCount,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [Parameter(Mandatory)][ValidateCount(1, 100)][ValidateSet('native', 'rust')][string[]]$BackendSequence
)

$ErrorActionPreference = 'Stop'

function Resolve-AbsolutePath {
    param([Parameter(Mandatory)][string]$Path)
    [System.IO.Path]::GetFullPath($Path)
}

function Invoke-SnapshotRestore {
    param(
        [Parameter(Mandatory)][string]$Client,
        [Parameter(Mandatory)][string]$CredentialFile,
        [Parameter(Mandatory)][string]$DatabaseName,
        [Parameter(Mandatory)][string]$SnapshotPath
    )

    $mysqlArguments = @(
        "--defaults-extra-file=$CredentialFile",
        "--database=$DatabaseName",
        '--default-character-set=utf8mb4',
        '--binary-mode'
    )
    $oldFakeArguments = $env:EXTDB3_SNAPSHOT_FAKE_CLIENT_ARGS
    $oldFakeClient = $env:EXTDB3_SNAPSHOT_FAKE_CLIENT
    try {
        if ([System.IO.Path]::GetExtension($Client) -ieq '.ps1') {
            $pwsh = (Get-Command pwsh -ErrorAction Stop).Source
            $env:EXTDB3_SNAPSHOT_FAKE_CLIENT_ARGS = ConvertTo-Json -InputObject $mysqlArguments -Compress
            $env:EXTDB3_SNAPSHOT_FAKE_CLIENT = '1'
            $clientExecutable = $pwsh
            $clientArguments = @('-NoLogo', '-NoProfile', '-NonInteractive', '-File', $Client)
        }
        else {
            $clientExecutable = $Client
            $clientArguments = $mysqlArguments
        }

        Get-Content -LiteralPath $SnapshotPath -Encoding utf8 -ReadCount 256 |
            & $clientExecutable @clientArguments *> $null
        $exitCode = $LASTEXITCODE
        if ($exitCode -ne 0) {
            throw "MySQL snapshot restore failed with exit code $exitCode."
        }
    }
    finally {
        if ($null -eq $oldFakeArguments) { Remove-Item Env:\EXTDB3_SNAPSHOT_FAKE_CLIENT_ARGS -ErrorAction SilentlyContinue }
        else { $env:EXTDB3_SNAPSHOT_FAKE_CLIENT_ARGS = $oldFakeArguments }
        if ($null -eq $oldFakeClient) { Remove-Item Env:\EXTDB3_SNAPSHOT_FAKE_CLIENT -ErrorAction SilentlyContinue }
        else { $env:EXTDB3_SNAPSHOT_FAKE_CLIENT = $oldFakeClient }
    }
}

function Get-RequiredNumber {
    param(
        [Parameter(Mandatory)][object]$Object,
        [Parameter(Mandatory)][string]$Property
    )
    [double]$number = 0
    $value = $Object.PSObject.Properties[$Property]
    $parsed = $false
    if ($null -ne $value -and $null -ne $value.Value) {
        $parsed = [double]::TryParse(
            [string]$value.Value,
            [System.Globalization.NumberStyles]::Float,
            [System.Globalization.CultureInfo]::InvariantCulture,
            [ref]$number
        )
    }
    if (-not $parsed -or [double]::IsNaN($number) -or [double]::IsInfinity($number)) {
        throw "Runner result is missing numeric property '$Property'."
    }
    $number
}

function Get-RequiredInteger {
    param(
        [Parameter(Mandatory)][object]$Object,
        [Parameter(Mandatory)][string]$Property
    )
    $number = Get-RequiredNumber -Object $Object -Property $Property
    if ($number -lt 0 -or $number -gt [int]::MaxValue -or $number -ne [math]::Truncate($number)) {
        throw "Runner result has an invalid work count '$Property'."
    }
    [int]$number
}

function Get-ExtensionLoadPath {
    param([Parameter(Mandatory)][string]$RptPath)
    if (-not (Test-Path -LiteralPath $RptPath -PathType Leaf)) {
        throw 'Runner result references a missing RPT file.'
    }
    $loadLine = Select-String -LiteralPath $RptPath -Pattern 'CallExtension loaded:\s*extDB3' | Select-Object -First 1
    if ($null -eq $loadLine) {
        throw 'Runner RPT does not contain the extDB3 load line.'
    }
    $match = [regex]::Match($loadLine.Line, '(?<path>[A-Z]:\\[^\r\n]*?extDB3_x64\.dll)', [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)
    if (-not $match.Success) {
        throw 'Runner RPT extDB3 load line does not contain a Windows DLL path.'
    }
    $match.Groups['path'].Value.Trim()
}

function Read-RunnerResult {
    param(
        [Parameter(Mandatory)][string]$ResultPath,
        [Parameter(Mandatory)][string]$ExpectedBackend
    )

    if (-not (Test-Path -LiteralPath $ResultPath -PathType Leaf)) {
        throw 'Runner did not write its JSON result file.'
    }
    try {
        $document = Get-Content -LiteralPath $ResultPath -Raw | ConvertFrom-Json -Depth 20
    }
    catch {
        throw 'Runner result is not valid JSON.'
    }
    if ($null -eq $document.Runs -or @($document.Runs).Count -ne 1) {
        throw 'Runner result must contain exactly one run.'
    }

    $run = @($document.Runs)[0]
    if ([string]$run.Backend -ine $ExpectedBackend) {
        throw 'Runner result backend does not match the requested backend.'
    }
    $seconds = Get-RequiredNumber -Object $run -Property 'WorldInitializeSeconds'
    if ($seconds -le 0) { throw 'Runner result has an invalid WorldInitialize duration.' }
    $counts = [ordered]@{
        databaseQueryCallLines = Get-RequiredInteger -Object $run -Property 'DatabaseQueryCallLines'
        successResultLines = Get-RequiredInteger -Object $run -Property 'SuccessResultLines'
        zeroResultLines = Get-RequiredInteger -Object $run -Property 'ZeroResultLines'
        databaseOrProtocolErrorLines = Get-RequiredInteger -Object $run -Property 'DatabaseOrProtocolErrorLines'
    }
    $dllLoadPath = Get-ExtensionLoadPath -RptPath ([string]$run.Rpt)

    $completedProperty = $run.PSObject.Properties['Completed']
    [pscustomobject]@{
        backend = $ExpectedBackend
        worldInitializeSeconds = [math]::Round($seconds, 6)
        counts = $counts
        errors = $counts.databaseOrProtocolErrorLines
        dllLoadPath = $dllLoadPath
        completed = ($null -ne $completedProperty -and $completedProperty.Value -eq $true)
    }
}

$runnerPath = Resolve-AbsolutePath $RunnerScript
$mysqlPath = Resolve-AbsolutePath $MySqlClient
$credentialPath = Resolve-AbsolutePath $DefaultsExtraFile
$snapshotPath = Resolve-AbsolutePath $SnapshotSql
$outputRoot = [System.IO.Path]::GetFullPath($OutputDirectory)
$backendSequence = @($BackendSequence | ForEach-Object { ([string]$_).ToLowerInvariant() })
if ($RunCount -lt $backendSequence.Count) {
    throw 'RunCount must be at least the number of entries in BackendSequence.'
}
New-Item -ItemType Directory -Path $outputRoot -Force | Out-Null
$runResults = [System.Collections.Generic.List[object]]::new()
$countBaseline = $null
$countsStable = $true
$failure = $null
$finalRestoreSucceeded = $false

try {
    for ($index = 0; $index -lt $RunCount; $index++) {
        $backend = $backendSequence[$index % $backendSequence.Count]
        Invoke-SnapshotRestore -Client $mysqlPath -CredentialFile $credentialPath -DatabaseName $Database -SnapshotPath $snapshotPath

        $runnerResultPath = Join-Path $outputRoot ('.runner-{0:D3}-{1}.json' -f ($index + 1), [guid]::NewGuid().ToString('N'))
        try {
            $pwsh = (Get-Command pwsh -ErrorAction Stop).Source
            & $pwsh -NoLogo -NoProfile -NonInteractive -File $runnerPath -RunsPerBackend 1 -StartRun 1 -BackendSelection $backend -OutputFile $runnerResultPath *> $null
            if ($LASTEXITCODE -ne 0) {
                throw "Runner failed with exit code $LASTEXITCODE."
            }
            $runResult = Read-RunnerResult -ResultPath $runnerResultPath -ExpectedBackend $backend
        }
        finally {
            Remove-Item -LiteralPath $runnerResultPath -Force -ErrorAction SilentlyContinue
        }

        $runResults.Add($runResult)
        if (-not $runResult.completed) {
            throw 'Runner marked the startup run incomplete.'
        }
        if ($runResult.counts.databaseQueryCallLines -eq 0 -or $runResult.counts.successResultLines -eq 0) {
            throw 'Runner reported no SQL_CUSTOM calls or successful results.'
        }
        if ($runResult.errors -ne 0) {
            throw 'Runner reported database or protocol errors.'
        }
        if ($null -eq $countBaseline) {
            $countBaseline = $runResult.counts
        }
        elseif ((ConvertTo-Json -InputObject $runResult.counts -Compress) -cne (ConvertTo-Json -InputObject $countBaseline -Compress)) {
            $countsStable = $false
            throw 'Runner work counts changed between snapshot-restored runs.'
        }
    }
}
catch {
    $failure = $_.Exception.Message
}
finally {
    try {
        Invoke-SnapshotRestore -Client $mysqlPath -CredentialFile $credentialPath -DatabaseName $Database -SnapshotPath $snapshotPath
        $finalRestoreSucceeded = $true
    }
    catch {
        if ($null -eq $failure) { $failure = 'Final MySQL snapshot restore failed.' }
        else { $failure = "$failure Final MySQL snapshot restore also failed." }
    }
}

$completedResults = @($runResults | Where-Object completed)
$times = @($completedResults | ForEach-Object { [double]$_.worldInitializeSeconds } | Sort-Object)
$performance = $null
if ($times.Count -gt 0) {
    $middle = [int][math]::Floor($times.Count / 2)
    if (($times.Count % 2) -eq 0) { $median = ($times[$middle - 1] + $times[$middle]) / 2 }
    else { $median = $times[$middle] }
    $performance = [ordered]@{
        medianWorldInitializeSeconds = [math]::Round($median, 6)
        minWorldInitializeSeconds = [math]::Round($times[0], 6)
        maxWorldInitializeSeconds = [math]::Round($times[-1], 6)
    }
}
$summary = [ordered]@{
    succeeded = ($null -eq $failure -and $finalRestoreSucceeded -and $completedResults.Count -eq $RunCount)
    requestedRunCount = $RunCount
    completedRunCount = $completedResults.Count
    backendSequence = $backendSequence
    countsStayedStable = $countsStable
    finalSnapshotRestoreSucceeded = $finalRestoreSucceeded
    performance = $performance
    runs = @($runResults)
    failure = $failure
}
$summaryJson = ConvertTo-Json -InputObject $summary -Depth 10 -Compress
Set-Content -LiteralPath (Join-Path $outputRoot 'summary.json') -Value $summaryJson -Encoding utf8NoBOM
Write-Output $summaryJson
if (-not $summary.succeeded) {
    throw 'Snapshot benchmark failed; see summary.json for completed-run status.'
}
