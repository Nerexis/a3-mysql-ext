$ErrorActionPreference = 'Stop'
$harnessPath = Join-Path $PSScriptRoot 'startup-snapshot-benchmark.ps1'
$testRoot = Join-Path ([System.IO.Path]::GetFullPath($env:TEMP)) "extdb3-snapshot-harness-tests-$PID"
$tempRoot = [System.IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
if (-not ([System.IO.Path]::GetFullPath($testRoot).StartsWith($tempRoot, [System.StringComparison]::OrdinalIgnoreCase))) {
    throw 'Test directory escaped the Windows temporary directory.'
}
if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
New-Item -ItemType Directory -Path $testRoot | Out-Null

try {
    $fakeClientPath = Join-Path $testRoot 'fake-mysql-client.ps1'
    $fakeRunnerPath = Join-Path $testRoot 'fake-startup-runner.ps1'
    $snapshotPath = Join-Path $testRoot 'snapshot.sql'
    $defaultsPath = Join-Path $testRoot 'mysql-options.cnf'
    $eventsPath = Join-Path $testRoot 'events.jsonl'
    $oldEvents = $env:EXTDB3_SNAPSHOT_FAKE_EVENTS
    $oldRestoreFail = $env:EXTDB3_SNAPSHOT_FAKE_RESTORE_FAIL_AT
    $oldRunnerFail = $env:EXTDB3_SNAPSHOT_FAKE_RUNNER_FAIL
    $oldUnstable = $env:EXTDB3_SNAPSHOT_FAKE_UNSTABLE
    $oldErrors = $env:EXTDB3_SNAPSHOT_FAKE_ERRORS
    $oldIncomplete = $env:EXTDB3_SNAPSHOT_FAKE_INCOMPLETE
    Set-Content -LiteralPath $snapshotPath -Value 'snapshot-test-payload' -Encoding utf8NoBOM
    Set-Content -LiteralPath $defaultsPath -Value 'credential-file-content-must-not-be-read' -Encoding utf8NoBOM

    @'
$ErrorActionPreference = 'Stop'
if ($env:EXTDB3_SNAPSHOT_FAKE_CLIENT -ne '1') { exit 60 }
$mysqlArguments = @(ConvertFrom-Json -InputObject $env:EXTDB3_SNAPSHOT_FAKE_CLIENT_ARGS)
if ($mysqlArguments.Count -ne 4 -or $mysqlArguments[0] -notmatch '^--defaults-extra-file=' -or $mysqlArguments[1] -ne '--database=exile_test') { exit 61 }
$credentialPath = $mysqlArguments[0].Substring('--defaults-extra-file='.Length)
if (-not (Test-Path -LiteralPath $credentialPath -PathType Leaf)) { exit 62 }
$sql = [Console]::In.ReadToEnd()
if ($sql -notmatch 'snapshot-test-payload') { exit 63 }
$eventsPath = $env:EXTDB3_SNAPSHOT_FAKE_EVENTS
$events = @(Get-Content -LiteralPath $eventsPath | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object { ConvertFrom-Json $_ })
$restoreNumber = @($events | Where-Object kind -eq 'restore').Count + 1
$failsAt = [int]$env:EXTDB3_SNAPSHOT_FAKE_RESTORE_FAIL_AT
$event = [pscustomobject]@{ kind = 'restore'; number = $restoreNumber; database = 'exile_test'; defaultsFirst = $true; snapshot = 'snapshot-test-payload'; succeeded = ($restoreNumber -ne $failsAt) }
Add-Content -LiteralPath $eventsPath -Value ($event | ConvertTo-Json -Compress) -Encoding utf8
if ($restoreNumber -eq $failsAt) { exit 64 }
exit 0
'@ | Set-Content -LiteralPath $fakeClientPath -Encoding utf8NoBOM

    @'
param(
    [int]$RunsPerBackend,
    [int]$StartRun,
    [string]$BackendSelection,
    [string]$OutputFile
)
$ErrorActionPreference = 'Stop'
if ($RunsPerBackend -ne 1 -or $StartRun -ne 1) { exit 70 }
$eventsPath = $env:EXTDB3_SNAPSHOT_FAKE_EVENTS
$events = @(Get-Content -LiteralPath $eventsPath | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object { ConvertFrom-Json $_ })
if ($events.Count -eq 0 -or $events[-1].kind -ne 'restore' -or -not $events[-1].succeeded) { exit 71 }
$runnerNumber = @($events | Where-Object kind -eq 'runner').Count + 1
$event = [pscustomobject]@{ kind = 'runner'; number = $runnerNumber; backend = $BackendSelection }
Add-Content -LiteralPath $eventsPath -Value ($event | ConvertTo-Json -Compress) -Encoding utf8
if ($env:EXTDB3_SNAPSHOT_FAKE_RUNNER_FAIL -eq [string]$runnerNumber) { exit 72 }
$calls = 11
if ($env:EXTDB3_SNAPSHOT_FAKE_UNSTABLE -eq '1' -and $runnerNumber -gt 1) { $calls = 12 }
$errors = 0
if ($env:EXTDB3_SNAPSHOT_FAKE_ERRORS -eq '1') { $errors = 1 }
$completed = $true
if ($env:EXTDB3_SNAPSHOT_FAKE_INCOMPLETE -eq '1') { $completed = $false }
$dllPath = 'C:\server\mods\@extdb3\extDB3_x64.dll'
if ($BackendSelection -ieq 'Rust') { $dllPath = 'E:\workspace\dist\extDB3_x64.dll' }
$rptPath = "$OutputFile.rpt"
"CallExtension loaded: extDB3 ($dllPath)" | Set-Content -LiteralPath $rptPath -Encoding utf8NoBOM
$seconds = 9
if ($BackendSelection -ieq 'Rust') { $seconds = 8 }
$run = [pscustomobject]@{
    Run = $runnerNumber
    Backend = $BackendSelection
    Rpt = $rptPath
    WorldInitializeSeconds = $seconds
    DatabaseQueryCallLines = $calls
    SuccessResultLines = 6
    ZeroResultLines = 2
    DatabaseOrProtocolErrorLines = $errors
    Completed = $completed
}
[pscustomobject]@{ Runs = @($run) } | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $OutputFile -Encoding utf8NoBOM
exit 0
'@ | Set-Content -LiteralPath $fakeRunnerPath -Encoding utf8NoBOM

    $env:EXTDB3_SNAPSHOT_FAKE_EVENTS = $eventsPath

    function Reset-TestEvents {
        Set-Content -LiteralPath $eventsPath -Value '' -Encoding utf8NoBOM
        Remove-Item Env:\EXTDB3_SNAPSHOT_FAKE_RESTORE_FAIL_AT, Env:\EXTDB3_SNAPSHOT_FAKE_RUNNER_FAIL, Env:\EXTDB3_SNAPSHOT_FAKE_UNSTABLE, Env:\EXTDB3_SNAPSHOT_FAKE_ERRORS, Env:\EXTDB3_SNAPSHOT_FAKE_INCOMPLETE -ErrorAction SilentlyContinue
    }

    function Invoke-HarnessExpectSuccess {
        param([string]$Name, [int]$Runs, [string[]]$Backends)
        $outputPath = Join-Path $testRoot $Name
        $output = & $harnessPath -RunnerScript $fakeRunnerPath -MySqlClient $fakeClientPath -DefaultsExtraFile $defaultsPath -Database 'exile_test' -SnapshotSql $snapshotPath -RunCount $Runs -OutputDirectory $outputPath -BackendSequence $Backends
        $summary = ($output | ConvertFrom-Json -Depth 10)
        if (-not $summary.succeeded) { throw "$Name unexpectedly reported failure." }
        $summary
    }

    function Invoke-HarnessExpectFailure {
        param([string]$Name, [int]$Runs, [string[]]$Backends)
        $outputPath = Join-Path $testRoot $Name
        $caught = $false
        try {
            $null = & $harnessPath -RunnerScript $fakeRunnerPath -MySqlClient $fakeClientPath -DefaultsExtraFile $defaultsPath -Database 'exile_test' -SnapshotSql $snapshotPath -RunCount $Runs -OutputDirectory $outputPath -BackendSequence $Backends
        }
        catch { $caught = $true }
        if (-not $caught) { throw "$Name unexpectedly succeeded." }
        Get-Content -LiteralPath (Join-Path $outputPath 'summary.json') -Raw | ConvertFrom-Json -Depth 10
    }

    Reset-TestEvents
    $summary = Invoke-HarnessExpectSuccess -Name 'alternating' -Runs 4 -Backends @('native', 'rust')
    if ($summary.completedRunCount -ne 4 -or -not $summary.countsStayedStable -or -not $summary.finalSnapshotRestoreSucceeded) { throw 'Alternating run summary was incomplete.' }
    if ($summary.performance.medianWorldInitializeSeconds -ne 8.5 -or $summary.performance.minWorldInitializeSeconds -ne 8 -or $summary.performance.maxWorldInitializeSeconds -ne 9) { throw 'WorldInitialize aggregation was incorrect.' }
    if ($summary.runs[1].backend -ne 'rust' -or $summary.runs[1].dllLoadPath -ne 'E:\workspace\dist\extDB3_x64.dll') { throw 'Rust DLL load path was not extracted from the RPT.' }
    $events = @(Get-Content -LiteralPath $eventsPath | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object { ConvertFrom-Json $_ })
    if ($events.Count -ne 9 -or $events[-1].kind -ne 'restore' -or $events[0].kind -ne 'restore') { throw "Expected a restore before each run and one final restore; found $($events.Count) events: $((@($events | ForEach-Object { $_.kind }) -join ',' ))." }
    for ($i = 0; $i -lt 4; $i++) {
        if ($events[$i * 2].kind -ne 'restore' -or $events[$i * 2 + 1].kind -ne 'runner') { throw 'Restore did not precede its startup run.' }
        $expectedBackend = @('native', 'rust')[$i % 2]
        if ($events[$i * 2 + 1].backend -ine $expectedBackend) { throw 'Backend sequence did not alternate.' }
    }
    if (@($events | Where-Object { $_.kind -eq 'restore' -and $_.snapshot -ne 'snapshot-test-payload' }).Count -ne 0) { throw 'A restore did not use the same snapshot file.' }

    Reset-TestEvents
    $env:EXTDB3_SNAPSHOT_FAKE_UNSTABLE = '1'
    $summary = Invoke-HarnessExpectFailure -Name 'unstable-counts' -Runs 3 -Backends @('native', 'rust')
    if ($summary.countsStayedStable -or $summary.completedRunCount -ne 2 -or -not $summary.finalSnapshotRestoreSucceeded) { throw 'Changed work counts were not rejected and restored.' }
    $events = @(Get-Content -LiteralPath $eventsPath | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object { ConvertFrom-Json $_ })
    if ($events[-1].kind -ne 'restore') { throw 'Final restore was skipped after work-count drift.' }

    Reset-TestEvents
    $env:EXTDB3_SNAPSHOT_FAKE_RUNNER_FAIL = '1'
    $summary = Invoke-HarnessExpectFailure -Name 'runner-failure' -Runs 2 -Backends @('native', 'rust')
    if ($summary.succeeded -or -not $summary.finalSnapshotRestoreSucceeded) { throw 'Runner failure was not reported or final restore was skipped.' }
    $events = @(Get-Content -LiteralPath $eventsPath | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object { ConvertFrom-Json $_ })
    if ($events[-1].kind -ne 'restore') { throw 'Final restore was skipped after a runner failure.' }

    Reset-TestEvents
    $env:EXTDB3_SNAPSHOT_FAKE_ERRORS = '1'
    $summary = Invoke-HarnessExpectFailure -Name 'reported-errors' -Runs 2 -Backends @('native', 'rust')
    if ($summary.succeeded -or $summary.runs[0].errors -ne 1 -or -not $summary.finalSnapshotRestoreSucceeded) { throw 'Database or protocol errors were not rejected.' }

    Reset-TestEvents
    $env:EXTDB3_SNAPSHOT_FAKE_INCOMPLETE = '1'
    $summary = Invoke-HarnessExpectFailure -Name 'incomplete-run' -Runs 1 -Backends @('native')
    if ($summary.succeeded -or $summary.completedRunCount -ne 0 -or $summary.runs[0].completed -or -not $summary.finalSnapshotRestoreSucceeded) { throw 'Incomplete startup was not rejected or recorded.' }

    Reset-TestEvents
    $env:EXTDB3_SNAPSHOT_FAKE_RESTORE_FAIL_AT = '1'
    $summary = Invoke-HarnessExpectFailure -Name 'restore-failure' -Runs 1 -Backends @('native')
    if ($summary.succeeded -or -not $summary.finalSnapshotRestoreSucceeded) { throw 'Restore failure was not reported while retrying final restoration.' }
    $events = @(Get-Content -LiteralPath $eventsPath | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object { ConvertFrom-Json $_ })
    if ($events.Count -ne 2 -or $events[-1].kind -ne 'restore' -or -not $events[-1].succeeded) { throw 'Harness did not retry the final snapshot restore after an initial failure.' }

    Reset-TestEvents
    $env:EXTDB3_SNAPSHOT_FAKE_RESTORE_FAIL_AT = '2'
    $summary = Invoke-HarnessExpectFailure -Name 'final-restore-failure' -Runs 1 -Backends @('native')
    if ($summary.succeeded -or $summary.finalSnapshotRestoreSucceeded) { throw 'Failed final restoration was not reported.' }

    'PASS startup snapshot harness: ordering, backend sequence, aggregation, work-count drift, runner errors, database errors, incomplete runs, initial/final restore failures, and final restoration.'
}
finally {
    if ($null -eq $oldEvents) { Remove-Item Env:\EXTDB3_SNAPSHOT_FAKE_EVENTS -ErrorAction SilentlyContinue }
    else { $env:EXTDB3_SNAPSHOT_FAKE_EVENTS = $oldEvents }
    if ($null -eq $oldRestoreFail) { Remove-Item Env:\EXTDB3_SNAPSHOT_FAKE_RESTORE_FAIL_AT -ErrorAction SilentlyContinue }
    else { $env:EXTDB3_SNAPSHOT_FAKE_RESTORE_FAIL_AT = $oldRestoreFail }
    if ($null -eq $oldRunnerFail) { Remove-Item Env:\EXTDB3_SNAPSHOT_FAKE_RUNNER_FAIL -ErrorAction SilentlyContinue }
    else { $env:EXTDB3_SNAPSHOT_FAKE_RUNNER_FAIL = $oldRunnerFail }
    if ($null -eq $oldUnstable) { Remove-Item Env:\EXTDB3_SNAPSHOT_FAKE_UNSTABLE -ErrorAction SilentlyContinue }
    else { $env:EXTDB3_SNAPSHOT_FAKE_UNSTABLE = $oldUnstable }
    if ($null -eq $oldErrors) { Remove-Item Env:\EXTDB3_SNAPSHOT_FAKE_ERRORS -ErrorAction SilentlyContinue }
    else { $env:EXTDB3_SNAPSHOT_FAKE_ERRORS = $oldErrors }
    if ($null -eq $oldIncomplete) { Remove-Item Env:\EXTDB3_SNAPSHOT_FAKE_INCOMPLETE -ErrorAction SilentlyContinue }
    else { $env:EXTDB3_SNAPSHOT_FAKE_INCOMPLETE = $oldIncomplete }
    Remove-Item -LiteralPath $testRoot -Recurse -Force
}
