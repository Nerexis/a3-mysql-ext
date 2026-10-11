[CmdletBinding()]
param(
    [string]$LinuxDistribution = 'Ubuntu'
)

$ErrorActionPreference = 'Stop'
$configDir = $PSScriptRoot
$certDir = Join-Path $configDir 'certs'
New-Item -ItemType Directory -Path $certDir -Force | Out-Null
$requiredCertificateFiles = @('ca-key.pem', 'ca.pem', 'server-key.pem', 'server.pem', 'wrong-ca-key.pem', 'wrong-ca.pem')
$missingCertificateFiles = @(
    $requiredCertificateFiles | Where-Object {
        -not (Test-Path -LiteralPath (Join-Path $certDir $_) -PathType Leaf)
    }
)
$wslCertDir = (& wsl.exe --distribution $LinuxDistribution --exec wslpath -a $certDir).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($wslCertDir)) {
    throw 'Could not resolve the test certificate directory inside Ubuntu WSL.'
}
if ($missingCertificateFiles.Count -gt 0) {
    $certificateCommand = "set -e; openssl req -x509 -newkey rsa:2048 -nodes -keyout '$wslCertDir/ca-key.pem' -out '$wslCertDir/ca.pem' -days 365 -subj '/CN=Exile extDB3 test CA' >/dev/null 2>&1; openssl req -newkey rsa:2048 -nodes -keyout '$wslCertDir/server-key.pem' -out '$wslCertDir/server.csr' -subj '/CN=localhost' >/dev/null 2>&1; printf '%s\n' 'subjectAltName=DNS:localhost,IP:127.0.0.1' 'extendedKeyUsage=serverAuth' 'basicConstraints=CA:FALSE' > '$wslCertDir/server.ext'; openssl x509 -req -in '$wslCertDir/server.csr' -CA '$wslCertDir/ca.pem' -CAkey '$wslCertDir/ca-key.pem' -CAcreateserial -out '$wslCertDir/server.pem' -days 365 -extfile '$wslCertDir/server.ext' >/dev/null 2>&1; openssl req -x509 -newkey rsa:2048 -nodes -keyout '$wslCertDir/wrong-ca-key.pem' -out '$wslCertDir/wrong-ca.pem' -days 365 -subj '/CN=Exile extDB3 unrelated test CA' >/dev/null 2>&1"
    wsl.exe --distribution $LinuxDistribution --exec bash -lc $certificateCommand
    if ($LASTEXITCODE -ne 0) { throw 'Could not create the isolated MySQL test certificates.' }
}
$composeArguments = @('--project-directory', $configDir, '--file', (Join-Path $configDir 'compose.yaml'), 'up', '--detach', '--wait')
if ($missingCertificateFiles.Count -gt 0) { $composeArguments += '--force-recreate' }
docker compose @composeArguments
if ($LASTEXITCODE -ne 0) { throw 'Could not start the isolated MySQL 9.7.2 container.' }

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $configDir '..\..'))
$sqlCustomDir = Join-Path $configDir 'sql_custom'
New-Item -ItemType Directory -Force -Path $sqlCustomDir | Out-Null
Copy-Item -LiteralPath (Join-Path $configDir 'extdb3-conf.ini.example') -Destination (Join-Path $configDir 'extdb3-conf.ini') -Force
Copy-Item -LiteralPath (Join-Path $repoRoot 'tests\fixtures\probe.ini') -Destination (Join-Path $sqlCustomDir 'probe.ini') -Force

docker exec exile-extdb3-mysql97-test mysql --protocol=TCP --host=127.0.0.1 --port=3306 --ssl-mode=VERIFY_IDENTITY --ssl-ca=/etc/mysql/certs/ca.pem --user=root --password=extdb_root_test_only --execute "SELECT VERSION(); SELECT user,plugin,ssl_type FROM mysql.user WHERE user = 'extdb_test';"
if ($LASTEXITCODE -ne 0) { throw 'MySQL readiness or authentication setup failed.' }
