$ErrorActionPreference = 'Stop'
$configDir = $PSScriptRoot
$certDir = Join-Path $configDir 'certs'
New-Item -ItemType Directory -Path $certDir -Force | Out-Null
$wslCertDir = '/mnt/e/Projekty/a3-mysql-ext/.audit/mysql9/certs'
wsl -d Ubuntu -- bash -lc "openssl req -x509 -newkey rsa:2048 -nodes -keyout '$wslCertDir/ca-key.pem' -out '$wslCertDir/ca.pem' -days 365 -subj '/CN=Exile extDB3 test CA' >/dev/null 2>&1; openssl req -newkey rsa:2048 -nodes -keyout '$wslCertDir/server-key.pem' -out '$wslCertDir/server.csr' -subj '/CN=localhost' >/dev/null 2>&1; printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\\nextendedKeyUsage=serverAuth\\nbasicConstraints=CA:FALSE\\n' > '$wslCertDir/server.ext'; openssl x509 -req -in '$wslCertDir/server.csr' -CA '$wslCertDir/ca.pem' -CAkey '$wslCertDir/ca-key.pem' -CAcreateserial -out '$wslCertDir/server.pem' -days 365 -extfile '$wslCertDir/server.ext' >/dev/null 2>&1"
if ($LASTEXITCODE -ne 0) { throw 'Could not create the isolated MySQL test certificates.' }
docker compose --file (Join-Path $configDir 'compose.yaml') up --detach --wait
if ($LASTEXITCODE -ne 0) { throw 'Could not start the isolated MySQL 9.7.2 container.' }
docker exec exile-extdb3-mysql97-test mysql --user=root --password=extdb_root_test_only --execute "SELECT VERSION(); SELECT user,plugin,ssl_type FROM mysql.user WHERE user = 'extdb_test';"
if ($LASTEXITCODE -ne 0) { throw 'MySQL readiness or authentication setup failed.' }
