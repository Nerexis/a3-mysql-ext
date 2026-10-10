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

pub fn database(root: &Path, section: &str) -> Result<Opts, String> {
    let text = std::fs::read_to_string(root.join("extdb3-conf.ini"))
        .map_err(|_| "Database Config Error".to_string())?;
    if text.len() > 1024 * 1024 {
        return Err("Database Config Error".into());
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
                return Err("Database Config Error".into());
            }
        }
    }
    let required = |key: &str| {
        entries
            .get(key)
            .cloned()
            .ok_or_else(|| "Database Config Error".to_string())
    };
    let host = required("ip")?;
    let port = entries
        .get("port")
        .map(|s| s.parse::<u16>())
        .transpose()
        .map_err(|_| "Database Config Error".to_string())?
        .unwrap_or(3306);
    let maximum = entries
        .get("max pool size")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|_| "Database Config Error".to_string())?
        .unwrap_or(16);
    let constraints = PoolConstraints::new(0, maximum)
        .filter(|_| (1..=64).contains(&maximum))
        .ok_or("Database Config Error")?;
    let ssl = tls_options(root, &entries, &host)?;
    Ok(OptsBuilder::default()
        .ip_or_hostname(host)
        .tcp_port(port)
        .user(Some(required("username")?))
        .pass(Some(required("password")?))
        .db_name(Some(required("database")?))
        .prefer_socket(false)
        .ssl_opts(Some(ssl))
        .pool_opts(PoolOpts::default().with_constraints(constraints))
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
        return Err("Error TLS Config".into());
    }

    let mut ssl = SslOpts::default();
    if let Some(ca) = entries.get("tls ca") {
        let path = root.join(ca);
        if !path.is_file() {
            return Err("Error TLS Config".into());
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
        Some(_) => Err("Error TLS Config".into()),
    }
}

pub fn filename(value: &str) -> Result<&str, String> {
    if value.is_empty()
        || value.len() > 128
        || value.contains(['/', '\\', ':'])
        || value == "."
        || value == ".."
    {
        Err("Error Invalid Format".into())
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "Error TLS Config"
        );
    }

    #[test]
    fn tls_cannot_be_disabled() {
        let entries = HashMap::from([("tls required".into(), "false".into())]);
        assert_eq!(
            tls_options(Path::new("."), &entries, "127.0.0.1").unwrap_err(),
            "Error TLS Config"
        );
    }
}
