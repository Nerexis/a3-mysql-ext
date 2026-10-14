#!/usr/bin/env bash
set -euo pipefail

config_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$config_dir/../.." && pwd)"
compose_file="$config_dir/compose.yaml"
cert_dir="$config_dir/certs"
sql_custom_dir="$config_dir/sql_custom"
mysql_port="${MYSQL9_PORT:-33307}"

if [[ ! "$mysql_port" =~ ^[0-9]{1,5}$ ]] || (( mysql_port < 1 || mysql_port > 65535 )); then
    printf 'MYSQL9_PORT must be between 1 and 65535.\n' >&2
    exit 2
fi

export COMPOSE_PROJECT_NAME="${COMPOSE_PROJECT_NAME:-exile-extdb3-mysql97-test}"
export MYSQL9_PORT="$mysql_port"
mkdir -p "$cert_dir" "$sql_custom_dir"

certificate_files=(ca-key.pem ca.pem server-key.pem server.pem wrong-ca-key.pem wrong-ca.pem)
certificates_ready=true
for file in "${certificate_files[@]}"; do
    if [[ ! -f "$cert_dir/$file" ]]; then
        certificates_ready=false
        break
    fi
done

if [[ "$certificates_ready" != true ]]; then
    openssl req -x509 -newkey rsa:2048 -nodes \
        -keyout "$cert_dir/ca-key.pem" -out "$cert_dir/ca.pem" \
        -days 365 -subj '/CN=Exile extDB3 test CA' >/dev/null 2>&1
    openssl req -newkey rsa:2048 -nodes \
        -keyout "$cert_dir/server-key.pem" -out "$cert_dir/server.csr" \
        -subj '/CN=localhost' >/dev/null 2>&1
    printf '%s\n' \
        'subjectAltName=DNS:localhost,IP:127.0.0.1' \
        'extendedKeyUsage=serverAuth' \
        'basicConstraints=CA:FALSE' > "$cert_dir/server.ext"
    openssl x509 -req -in "$cert_dir/server.csr" \
        -CA "$cert_dir/ca.pem" -CAkey "$cert_dir/ca-key.pem" \
        -CAcreateserial -out "$cert_dir/server.pem" -days 365 \
        -extfile "$cert_dir/server.ext" >/dev/null 2>&1
    openssl req -x509 -newkey rsa:2048 -nodes \
        -keyout "$cert_dir/wrong-ca-key.pem" -out "$cert_dir/wrong-ca.pem" \
        -days 365 -subj '/CN=Exile extDB3 unrelated test CA' >/dev/null 2>&1
fi

# This disposable server key is bind-mounted from the host and read by mysqld
# after the image drops privileges to its mysql user.
chmod 644 "$cert_dir/server-key.pem"

tr -d '\r' < "$config_dir/extdb3-conf.ini.example" \
    | sed "s/^Port = 33307$/Port = $mysql_port/" \
    > "$config_dir/extdb3-conf.ini"
cp "$repo_root/tests/fixtures/probe.ini" "$sql_custom_dir/probe.ini"
cp "$repo_root/tests/fixtures/exile.ini" "$sql_custom_dir/exile.ini"

if ! docker compose --project-directory "$config_dir" --file "$compose_file" up --detach --wait; then
    docker compose --project-directory "$config_dir" --file "$compose_file" logs --no-color mysql9 >&2 || true
    exit 1
fi
mysql_status="$(docker compose --project-directory "$config_dir" --file "$compose_file" exec --no-TTY mysql9 \
    mysql --protocol=TCP --host=127.0.0.1 --port=3306 \
    --ssl-mode=VERIFY_IDENTITY --ssl-ca=/etc/mysql/certs/ca.pem \
    --user=root --password=extdb_root_test_only \
    --batch --skip-column-names \
    --execute "SELECT VERSION(), (SELECT plugin FROM mysql.user WHERE user = 'extdb_test' LIMIT 1), (SELECT ssl_type FROM mysql.user WHERE user = 'extdb_test' LIMIT 1);")"
read -r mysql_version mysql_plugin mysql_ssl_type <<< "$mysql_status"
if [[ "$mysql_version" != 9.7.2* || "$mysql_plugin" != caching_sha2_password || "$mysql_ssl_type" != ANY ]]; then
    printf 'MySQL 9.7.2 TLS authentication setup is invalid: %s\n' "$mysql_status" >&2
    exit 1
fi

printf 'MySQL %s is ready with %s and REQUIRE SSL.\n' "$mysql_version" "$mysql_plugin"
