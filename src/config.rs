use crate::compat::error_code;
use mysql_async::{Opts, OptsBuilder, PoolConstraints, PoolOpts, SslOpts};
use std::{
    collections::HashMap,
    ffi::OsStr,
    net::IpAddr,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const CONFIG_FILE: &str = "extdb3-conf.ini";

pub struct Snapshot {
    text: Option<String>,
    config_path: Option<PathBuf>,
    pub thread: usize,
    pub allow_reset: bool,
    log_flush: bool,
}

impl Snapshot {
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = find_config_file(root);
        let text = match path.as_ref() {
            Some(path) => Some(
                std::fs::read_to_string(path)
                    .map_err(|_| error_code::DATABASE_CONFIG.to_string())?,
            ),
            None => None,
        };
        if text
            .as_ref()
            .is_some_and(|text| text.len() > MAX_CONFIG_BYTES)
        {
            return Err(error_code::DATABASE_CONFIG.into());
        }
        let mut main = HashMap::new();
        let mut log = HashMap::new();
        if let Some(text) = text.as_deref() {
            read_settings(text, "Main", &mut main)?;
            read_settings(text, "Log", &mut log)?;
        }

        let thread = main
            .get("thread")
            .map(|value| value.parse::<usize>())
            .transpose()
            .map_err(|_| error_code::DATABASE_CONFIG.to_string())?
            .unwrap_or(0);
        if thread != 0 && !(2..=6).contains(&thread) {
            return Err(error_code::DATABASE_CONFIG.into());
        }
        let allow_reset = setting_bool(&main, "allow reset", false)?;
        let randomize_config_file = setting_bool(&main, "randomize config file", false)?;
        let log_flush = setting_bool(&log, "flush", true)?;
        let config_path = match path {
            Some(path)
                if randomize_config_file
                    && path.file_name().and_then(OsStr::to_str) == Some(CONFIG_FILE) =>
            {
                let randomized_path = randomized_config_path(root)?;
                std::fs::rename(path, &randomized_path)
                    .map_err(|_| error_code::DATABASE_CONFIG.to_string())?;
                Some(randomized_path)
            }
            path => path,
        };
        Ok(Self {
            text,
            config_path,
            thread,
            allow_reset,
            log_flush,
        })
    }

    pub fn log_flush(&self) -> bool {
        self.log_flush
    }

    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    pub fn database(&self, root: &Path, section: &str) -> Result<Opts, String> {
        let text = self
            .text
            .as_deref()
            .ok_or_else(|| error_code::DATABASE_CONFIG.to_string())?;
        database_from_text(root, text, section)
    }
}

fn read_settings(
    text: &str,
    wanted: &str,
    entries: &mut HashMap<String, String>,
) -> Result<(), String> {
    let mut active = false;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with([';', '#']) {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            active = line[1..line.len() - 1].eq_ignore_ascii_case(wanted);
        } else if active {
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| error_code::DATABASE_CONFIG.to_string())?;
            let key = key.trim().to_ascii_lowercase();
            if entries.insert(key, value.trim().to_string()).is_some() {
                return Err(error_code::DATABASE_CONFIG.into());
            }
        }
    }
    Ok(())
}

fn setting_bool(
    entries: &HashMap<String, String>,
    key: &str,
    default: bool,
) -> Result<bool, String> {
    match entries.get(key).map(String::as_str) {
        None => Ok(default),
        Some(value) if value.eq_ignore_ascii_case("true") => Ok(true),
        Some(value) if value.eq_ignore_ascii_case("false") => Ok(false),
        Some(_) => Err(error_code::DATABASE_CONFIG.into()),
    }
}

pub fn root() -> PathBuf {
    if let Some(value) = std::env::var_os("EXTDB3_ROOT") {
        return PathBuf::from(value);
    }
    for path in [".", "@extdb3", "@extDB3", "mods/@extdb3", "mods/@extDB3"] {
        let root = PathBuf::from(path);
        if find_config_file(&root).is_some() {
            return root;
        }
    }
    PathBuf::from(".")
}

fn find_config_file(root: &Path) -> Option<PathBuf> {
    let standard = root.join(CONFIG_FILE);
    if standard.is_file() {
        return Some(standard);
    }

    let mut randomized = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_randomized_config_file(path))
        .map(|path| {
            let modified = std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH);
            (modified, path)
        })
        .collect::<Vec<_>>();
    randomized.sort_by_key(|(modified, _)| *modified);
    randomized.pop().map(|(_, path)| path)
}

fn is_randomized_config_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let Some(suffix) = name
        .strip_prefix("extdb3-conf-")
        .and_then(|name| name.strip_suffix(".ini"))
    else {
        return false;
    };
    suffix.len() == 8
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn randomized_config_path(root: &Path) -> Result<PathBuf, String> {
    const ALPHABET: &[u8; 36] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    for _ in 0..8 {
        let mut random = [0_u8; 8];
        getrandom::getrandom(&mut random).map_err(|_| error_code::DATABASE_CONFIG.to_string())?;
        let suffix = random
            .iter()
            .map(|value| char::from(ALPHABET[usize::from(*value) * ALPHABET.len() / 256]))
            .collect::<String>();
        let path = root.join(format!("extdb3-conf-{suffix}.ini"));
        if std::fs::symlink_metadata(&path).is_err() {
            return Ok(path);
        }
    }
    Err(error_code::DATABASE_CONFIG.into())
}

#[cfg(test)]
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

#[cfg(test)]
pub fn database(root: &Path, section: &str) -> Result<Opts, String> {
    let text = std::fs::read_to_string(root.join("extdb3-conf.ini"))
        .map_err(|_| error_code::DATABASE_CONFIG.to_string())?;
    database_from_text(root, &text, section)
}

fn database_from_text(root: &Path, text: &str, section: &str) -> Result<Opts, String> {
    if text.len() > MAX_CONFIG_BYTES {
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
