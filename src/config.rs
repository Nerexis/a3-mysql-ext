use crate::compat::error_code;
use mysql_async::{Opts, OptsBuilder, PoolConstraints, PoolOpts, SslOpts};
use std::{
    collections::HashMap,
    net::IpAddr,
    path::{Path, PathBuf},
    time::Duration,
};

pub fn root() -> PathBuf {
    if let Some(value) = std::env::var_os("EXTDB3_ROOT") {
        return PathBuf::from(value);
    }
    for path in [".", "@extdb3", "@extDB3", "mods/@extdb3", "mods/@extDB3"] {
        let root = PathBuf::from(path);
        if root.join("extdb3-conf.ini").is_file() {
            return root;
        }
    }
    PathBuf::from(".")
}

pub fn log_flush(root: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join("extdb3-conf.ini")) else {
        return true;
    };
    let mut in_log_section = false;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with([';', '#']) {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_log_section = line[1..line.len() - 1].eq_ignore_ascii_case("Log");
        } else if in_log_section
            && let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("Flush")
        {
            return !value.trim().eq_ignore_ascii_case("false");
        }
    }
    true
}

pub fn database(root: &Path, section: &str) -> Result<Opts, String> {
    let text = std::fs::read_to_string(root.join("extdb3-conf.ini"))
        .map_err(|_| error_code::DATABASE_CONFIG.to_string())?;
    if text.len() > 1024 * 1024 {
        return Err(error_code::DATABASE_CONFIG.into());
    }
    let mut active = false;
    let mut entries = HashMap::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with([';', '#']) || line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            active = &line[1..line.len() - 1] == section;
        } else if active && let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_ascii_lowercase();
            if entries.insert(key, value.trim().to_string()).is_some() {
                return Err(error_code::DATABASE_CONFIG.into());
            }
        }
    }
    let required = |key: &str| {
        entries
            .get(key)
            .cloned()
            .ok_or_else(|| error_code::DATABASE_CONFIG.to_string())
    };
    let host = required("ip")?;
    let port = entries
        .get("port")
        .map(|s| s.parse::<u16>())
        .transpose()
        .map_err(|_| error_code::DATABASE_CONFIG.to_string())?
        .unwrap_or(3306);
    let maximum = entries
        .get("max pool size")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|_| error_code::DATABASE_CONFIG.to_string())?
        .unwrap_or(16);
    let constraints = PoolConstraints::new(0, maximum)
        .filter(|_| (1..=64).contains(&maximum))
        .ok_or(error_code::DATABASE_CONFIG)?;
    let reset_connection = reset_connection_option(&entries)?;
    let ssl = tls_options(root, &entries, &host)?;
    Ok(OptsBuilder::default()
        .ip_or_hostname(host)
        .tcp_port(port)
        .user(Some(required("username")?))
        .pass(Some(required("password")?))
        .db_name(Some(required("database")?))
        .prefer_socket(false)
        .ssl_opts(Some(ssl))
        .pool_opts(
            PoolOpts::default()
                .with_constraints(constraints)
                .with_inactive_connection_ttl(Duration::from_secs(60))
                .with_reset_connection(reset_connection),
        )
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .into())
}

fn tls_options(
    root: &Path,
    entries: &HashMap<String, String>,
    host: &str,
) -> Result<SslOpts, String> {
    let required = tls_boolean(entries, "tls required", true)?;
    let verify = tls_boolean(entries, "tls verify", true)?;
    let verify_hostname = tls_boolean(entries, "tls verify hostname", true)?;
    if !required
        || ((!verify || !verify_hostname)
            && !host
                .parse::<IpAddr>()
                .is_ok_and(|address| address.is_loopback()))
    {
        return Err(error_code::TLS_CONFIG.into());
    }

    let mut ssl = SslOpts::default();
    if let Some(ca) = entries.get("tls ca") {
        let path = root.join(ca);
        if !path.is_file() {
            return Err(error_code::TLS_CONFIG.into());
        }
        ssl = ssl.with_root_certs(vec![path.into()]);
    }
    Ok(ssl
        .with_danger_accept_invalid_certs(!verify)
        .with_danger_skip_domain_validation(!verify_hostname))
}

fn tls_boolean(
    entries: &HashMap<String, String>,
    key: &str,
    default: bool,
) -> Result<bool, String> {
    match entries.get(key).map(String::as_str) {
        None => Ok(default),
        Some(value) if value.eq_ignore_ascii_case("true") => Ok(true),
        Some(value) if value.eq_ignore_ascii_case("false") => Ok(false),
        Some(_) => Err(error_code::TLS_CONFIG.into()),
    }
}

fn reset_connection_option(entries: &HashMap<String, String>) -> Result<bool, String> {
    match entries.get("reset connection").map(String::as_str) {
        None => Ok(true),
        Some(value) if value.eq_ignore_ascii_case("true") => Ok(true),
        Some(value) if value.eq_ignore_ascii_case("false") => Ok(false),
        Some(_) => Err(error_code::DATABASE_CONFIG.into()),
    }
}

pub fn filename(value: &str) -> Result<&str, String> {
    if value.is_empty()
        || value.len() > 128
        || value.contains(['/', '\\', ':'])
        || value == "."
        || value == ".."
    {
        Err(error_code::INVALID_FORMAT.into())
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TemporaryDirectory(PathBuf);

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temporary_config_root() -> TemporaryDirectory {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "exile-extdb3-config-test-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        TemporaryDirectory(path)
    }

    #[test]
    fn database_pool_retains_idle_connections_for_reuse() {
        let root = temporary_config_root();
        std::fs::write(
            root.0.join("extdb3-conf.ini"),
            "[Main]\nip=127.0.0.1\nusername=test\npassword=test\ndatabase=test\n",
        )
        .unwrap();

        let options = database(&root.0, "Main").unwrap();
        let idle_ttl = options.pool_opts().inactive_connection_ttl();

        assert!(
            idle_ttl > Duration::ZERO,
            "pool drops returned connections immediately when idle TTL is {idle_ttl:?}"
        );
        assert!(options.pool_opts().reset_connection());
    }

    #[test]
    fn database_can_disable_connection_reset_for_its_section() {
        let root = temporary_config_root();
        std::fs::write(
            root.0.join("extdb3-conf.ini"),
            "[Main]\nip=127.0.0.1\nusername=test\npassword=test\ndatabase=test\nReset Connection=FaLsE\n[Other]\nip=127.0.0.1\nusername=test\npassword=test\ndatabase=test\n",
        )
        .unwrap();

        assert!(
            !database(&root.0, "Main")
                .unwrap()
                .pool_opts()
                .reset_connection()
        );
        assert!(
            database(&root.0, "Other")
                .unwrap()
                .pool_opts()
                .reset_connection()
        );
    }

    #[test]
    fn database_rejects_invalid_connection_reset_value() {
        let root = temporary_config_root();
        std::fs::write(
            root.0.join("extdb3-conf.ini"),
            "[Main]\nip=127.0.0.1\nusername=test\npassword=test\ndatabase=test\nReset Connection=maybe\n",
        )
        .unwrap();

        assert_eq!(
            database(&root.0, "Main").unwrap_err(),
            "Database Config Error"
        );
    }

    #[test]
    fn log_flush_defaults_to_true_and_reads_the_log_section() {
        let root = temporary_config_root();
        assert!(log_flush(&root.0));

        std::fs::write(
            root.0.join("extdb3-conf.ini"),
            "[Main]\nFlush=false\n[Log]\nFlush=FaLsE\n",
        )
        .unwrap();
        assert!(!log_flush(&root.0));

        std::fs::write(root.0.join("extdb3-conf.ini"), "[Log]\nFlush=true\n").unwrap();
        assert!(log_flush(&root.0));
    }

    #[test]
    fn tls_verification_defaults_to_enabled() {
        let options = tls_options(Path::new("."), &HashMap::new(), "db.example.test").unwrap();
        assert!(!options.accept_invalid_certs());
        assert!(!options.skip_domain_validation());
    }

    #[test]
    fn tls_verification_can_be_skipped_for_loopback_only() {
        let entries = HashMap::from([
            ("tls required".into(), "true".into()),
            ("tls verify".into(), "false".into()),
            ("tls verify hostname".into(), "false".into()),
        ]);
        let options = tls_options(Path::new("."), &entries, "127.0.0.1").unwrap();
        assert!(options.accept_invalid_certs());
        assert!(options.skip_domain_validation());
    }

    #[test]
    fn tls_verification_cannot_be_skipped_for_remote_hosts() {
        let entries = HashMap::from([("tls verify".into(), "false".into())]);
        assert_eq!(
            tls_options(Path::new("."), &entries, "db.example.test").unwrap_err(),
            error_code::TLS_CONFIG
        );
    }

    #[test]
    fn tls_cannot_be_disabled() {
        let entries = HashMap::from([("tls required".into(), "false".into())]);
        assert_eq!(
            tls_options(Path::new("."), &entries, "127.0.0.1").unwrap_err(),
            error_code::TLS_CONFIG
        );
    }
}
