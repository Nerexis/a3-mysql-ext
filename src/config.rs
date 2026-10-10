use mysql_async::{Opts, OptsBuilder, PoolConstraints, PoolOpts, SslOpts};
use std::{
    collections::HashMap,
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
    let mut ssl = SslOpts::default();
    if let Some(ca) = entries.get("tls ca") {
        let path = root.join(ca);
        if !path.is_file() {
            return Err("Error TLS Config".into());
        }
        ssl = ssl.with_root_certs(vec![path.into()]);
    }
    for key in ["tls verify", "tls verify hostname", "tls required"] {
        if entries
            .get(key)
            .is_some_and(|value| !value.eq_ignore_ascii_case("true"))
        {
            return Err("Error TLS Config".into());
        }
    }
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
