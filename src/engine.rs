#[cfg(feature = "profile-requests")]
use crate::profile;
use crate::{
    callback,
    compat::{Failure, error_code},
    config::{self, Snapshot},
    sql_custom::{self, Custom},
};
use chrono::{
    DateTime, Datelike, Duration as DateDuration, Local, NaiveDate, NaiveDateTime, Timelike,
};
use mysql_async::Pool;
use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    sync::{Arc, Mutex, OnceLock, mpsc as blocking},
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinSet};

const QUEUE: usize = 256;
const MAX_ACTIVE_JOBS: usize = QUEUE;
const COMPLETION_QUEUE: usize = 16;
const RESULTS: usize = 128;
const CALLBACK_RETRY_INTERVAL: Duration = Duration::from_millis(50);
pub const MAX_INPUT: usize = 256 * 1024;
const MAX_RESULT: usize = 1024 * 1024;
const LOG_MAX_BYTES: u64 = 100 * 1024 * 1024;
const LOG_BACKUP_COUNT: usize = 3;

#[derive(Default)]
struct ReservationCounter(Arc<AtomicUsize>);

impl ReservationCounter {
    fn try_reserve(&self) -> Option<ResultReservation> {
        self.0
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < RESULTS).then_some(count + 1)
            })
            .ok()
            .map(|_| ResultReservation(Arc::clone(&self.0)))
    }
}

struct ResultReservation(Arc<AtomicUsize>);

impl Drop for ResultReservation {
    fn drop(&mut self) {
        let previous = self.0.fetch_sub(1, Ordering::Relaxed);
        debug_assert!(previous > 0);
    }
}

pub fn error(message: &str) -> String {
    format!("[0,\"{}\"]", message.replace('"', "\"\""))
}

struct Call {
    text: String,
    capacity: usize,
    reply: blocking::Sender<String>,
    #[cfg(feature = "profile-requests")]
    received: Option<Instant>,
}
struct Job {
    id: u64,
    protocol_type: &'static str,
    protocol: String,
    query_name: String,
    pool: Pool,
    operation: sql_custom::Operation,
    delivery: Delivery,
    runtime_log: Arc<RuntimeLog>,
    #[cfg(feature = "profile-requests")]
    profile: Option<profile::Request>,
}
enum Delivery {
    Discard,
    Saved(ResultReservation),
    Callback(ResultReservation),
    Sync {
        reply: blocking::Sender<String>,
        capacity: usize,
        reservation: ResultReservation,
    },
}
struct Completed {
    id: u64,
    delivery: Delivery,
    result: String,
    #[cfg(feature = "profile-requests")]
    profile: Option<profile::Request>,
    #[cfg(feature = "profile-requests")]
    worker_started: Option<Instant>,
    #[cfg(feature = "profile-requests")]
    worker_finished: Option<Instant>,
    #[cfg(feature = "profile-requests")]
    execution: Option<profile::Execution>,
}
struct CompletionTicket {
    id: u64,
    protocol_type: &'static str,
    protocol: String,
    query_name: String,
    delivery: Delivery,
    runtime_log: Arc<RuntimeLog>,
    #[cfg(feature = "profile-requests")]
    profile: Option<profile::Request>,
}
struct WorkerResult {
    result: String,
    #[cfg(feature = "profile-requests")]
    worker_started: Option<Instant>,
    #[cfg(feature = "profile-requests")]
    worker_finished: Option<Instant>,
    #[cfg(feature = "profile-requests")]
    execution: Option<profile::Execution>,
}
enum ResultState {
    Pending,
    Ready {
        text: String,
        offset: usize,
        _reservation: ResultReservation,
    },
}
enum Lock {
    Open,
    Permanent,
    Code(String),
}
enum Protocol {
    Sql {
        pool: Pool,
        definition: sql_custom::ProtocolDefinition,
    },
    Log(Option<String>),
}

struct RuntimeLog {
    log_directory: PathBuf,
    main_path: PathBuf,
    flush_on_info: bool,
    sinks: Mutex<HashMap<PathBuf, RotatingFile>>,
}

impl RuntimeLog {
    fn new(root: &Path, now: &DateTime<Local>, flush_on_info: bool) -> Self {
        let log_directory = dated_log_directory(root, now);
        Self {
            log_directory,
            main_path: main_log_path(root, now),
            flush_on_info,
            sinks: Mutex::new(HashMap::new()),
        }
    }

    fn write(&self, filename: Option<&str>, payload: &str) -> std::io::Result<()> {
        let now = Local::now();
        self.write_at(filename, payload, &now)
    }

    fn write_startup(&self, payload: &str) -> std::io::Result<()> {
        self.write_line(&self.main_path, payload)
    }

    fn write_at(
        &self,
        filename: Option<&str>,
        payload: &str,
        now: &DateTime<Local>,
    ) -> std::io::Result<()> {
        let path = match filename {
            Some(filename) => self.log_directory.join(filename),
            None => self.main_path.clone(),
        };
        let line = format_log_line(now, current_os_thread_id(), payload);
        self.write_line(&path, &line)
    }

    fn register(&self, filename: Option<&str>) -> std::io::Result<()> {
        let path = match filename {
            Some(filename) => self.log_directory.join(filename),
            None => self.main_path.clone(),
        };
        let mut sinks = self
            .sinks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let std::collections::hash_map::Entry::Vacant(entry) = sinks.entry(path) {
            let sink = RotatingFile::open(
                entry.key().clone(),
                self.flush_on_info,
                LOG_MAX_BYTES,
                LOG_BACKUP_COUNT,
            )?;
            entry.insert(sink);
        }
        Ok(())
    }

    fn write_line(&self, path: &Path, payload: &str) -> std::io::Result<()> {
        let mut line = Vec::with_capacity(payload.len() + 1);
        line.extend_from_slice(payload.as_bytes());
        line.push(b'\n');

        let mut sinks = self
            .sinks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let std::collections::hash_map::Entry::Vacant(entry) = sinks.entry(path.to_path_buf()) {
            let sink = RotatingFile::open(
                entry.key().clone(),
                self.flush_on_info,
                LOG_MAX_BYTES,
                LOG_BACKUP_COUNT,
            )?;
            entry.insert(sink);
        }
        sinks
            .get_mut(path)
            .expect("log sink inserted above")
            .write_line(&line)
    }

    fn write_event(&self, message: &str) {
        let detail = message.replace(['\r', '\n'], " ");
        if let Err(error) = self.write(None, &format!("extDB3 Rust: {detail}")) {
            eprintln!("extDB3 Rust: could not write runtime log: {error}");
        }
    }

    fn write_legacy_event(&self, message: &str) {
        let detail = message.replace(['\r', '\n'], " ");
        if let Err(error) = self.write(None, &format!("extDB3: {detail}")) {
            eprintln!("extDB3 Rust: could not write runtime log: {error}");
        }
    }
}

struct RotatingFile {
    path: PathBuf,
    output: Option<BufWriter<File>>,
    size: u64,
    flush_on_info: bool,
    max_bytes: u64,
    backup_count: usize,
}

impl RotatingFile {
    fn open(
        path: PathBuf,
        flush_on_info: bool,
        max_bytes: u64,
        backup_count: usize,
    ) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path,
            output: Some(BufWriter::new(file)),
            size,
            flush_on_info,
            max_bytes,
            backup_count,
        })
    }

    fn write_line(&mut self, line: &[u8]) -> std::io::Result<()> {
        self.ensure_open()?;
        if self.size > 0 && self.size.saturating_add(line.len() as u64) > self.max_bytes {
            self.rotate()?;
        }
        let output = self.output.as_mut().expect("log file opened above");
        output.write_all(line)?;
        self.size = self.size.saturating_add(line.len() as u64);
        if self.flush_on_info {
            output.flush()?;
        }
        Ok(())
    }

    fn ensure_open(&mut self) -> std::io::Result<()> {
        if self.output.is_none() {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            self.size = file.metadata()?.len();
            self.output = Some(BufWriter::new(file));
        }
        Ok(())
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        if let Some(mut output) = self.output.take() {
            if let Err(error) = output.flush() {
                self.output = Some(output);
                return Err(error);
            }
            drop(output);
        }

        if self.backup_count == 0 {
            if self.path.exists() {
                fs::remove_file(&self.path)?;
            }
        } else {
            for index in (1..=self.backup_count).rev() {
                let target = backup_path(&self.path, index);
                if target.exists() {
                    fs::remove_file(&target)?;
                }
                let source = if index == 1 {
                    self.path.clone()
                } else {
                    backup_path(&self.path, index - 1)
                };
                if source.exists() {
                    fs::rename(source, target)?;
                }
            }
        }

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        self.output = Some(BufWriter::new(file));
        self.size = 0;
        Ok(())
    }
}

fn backup_path(path: &Path, index: usize) -> PathBuf {
    let filename = path
        .file_name()
        .expect("log path has a filename")
        .to_string_lossy();
    path.with_file_name(format!("{filename}.{index}"))
}

fn format_log_line(now: &DateTime<Local>, thread_id: u64, payload: &str) -> String {
    #[cfg(windows)]
    let timestamp = now.format("%H:%M:%S:%6f %:z");
    #[cfg(not(windows))]
    let timestamp = now.format("%H:%M:%S %:z");
    format!("[{timestamp}] [Thread {thread_id}] {payload}")
}

#[cfg(target_os = "linux")]
fn current_os_thread_id() -> u64 {
    // SAFETY: gettid has no pointer arguments or preconditions.
    unsafe { libc::gettid() as u64 }
}

#[cfg(windows)]
fn current_os_thread_id() -> u64 {
    // SAFETY: GetCurrentThreadId has no pointer arguments or preconditions.
    unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() as u64 }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn current_os_thread_id() -> u64 {
    format!("{:?}", std::thread::current().id())
        .trim_start_matches("ThreadId(")
        .trim_end_matches(')')
        .parse()
        .unwrap_or_default()
}

fn selected_workers(configured: usize, detected: Option<usize>) -> usize {
    if configured == 0 {
        detected.unwrap_or(1).max(1)
    } else {
        configured
    }
}

fn startup_messages(
    root: &Path,
    config_file: Option<&Path>,
    configured: usize,
    cpus: Option<usize>,
    workers: usize,
) -> Vec<String> {
    let config_message = match config_file {
        Some(path)
            if path.file_name().and_then(|name| name.to_str()) == Some("extdb3-conf.ini") =>
        {
            "extDB3: Found extdb3-conf.ini".to_string()
        }
        Some(_) => "extDB3: Found randomized extdb3-conf.ini".to_string(),
        None => "extDB3: Unable to find extdb3-conf.ini".to_string(),
    };
    let config_path = config_file
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join("extdb3-conf.ini"));
    let platform = match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    };
    vec![
        String::new(),
        "extDB3: https://bitbucket.org/torndeco/extdb3/wiki/Home".into(),
        "extDB3: Version: 1.033".into(),
        format!("extDB3: {platform} Version"),
        format!("extDB3 Rust: Version: {}", env!("CARGO_PKG_VERSION")),
        format!(
            "extDB3 Rust: Platform: {platform} {}",
            std::env::consts::ARCH
        ),
        "extDB3 Rust: Using the new Rust runtime: Tokio multi-thread scheduler".into(),
        config_message,
        format!("extDB3 Rust: Configuration path: {}", config_path.display()),
        cpus.map_or_else(
            || format!("extDB3: CPU detection failed, Setting up {workers} Worker Threads"),
            |cpus| format!("extDB3: Detected {cpus} Cores, Setting up {workers} Worker Threads"),
        ),
        format!(
            "extDB3 Rust: Thread setting: {configured}, detected logical CPUs: {}, selected Tokio workers: {workers}",
            cpus.map_or_else(|| "unavailable".to_string(), |cpus| cpus.to_string())
        ),
        String::new(),
        String::new(),
    ]
}

fn system_command_label(command: &str) -> &str {
    let label = command.split(':').next().unwrap_or_default();
    if !label.is_empty()
        && label
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        label
    } else {
        "<invalid>"
    }
}

fn date_add(date: &str, offset: &str) -> Result<String, Failure> {
    let [year, month, day, hour, minute, second] = parse_integer_array::<6>(date, "DATEADD date")?;
    let [days, hours, minutes, seconds] = parse_integer_array::<4>(offset, "DATEADD offset")?;

    let year = i32::try_from(year)
        .map_err(|_| Failure::new(error_code::INVALID_FORMAT, "DATEADD year is out of range"))?;
    let month = u32::try_from(month)
        .map_err(|_| Failure::new(error_code::INVALID_FORMAT, "DATEADD month is out of range"))?;
    let day = u32::try_from(day)
        .map_err(|_| Failure::new(error_code::INVALID_FORMAT, "DATEADD day is out of range"))?;
    let hour = u32::try_from(hour)
        .map_err(|_| Failure::new(error_code::INVALID_FORMAT, "DATEADD hour is out of range"))?;
    let minute = u32::try_from(minute)
        .map_err(|_| Failure::new(error_code::INVALID_FORMAT, "DATEADD minute is out of range"))?;
    let second = u32::try_from(second)
        .map_err(|_| Failure::new(error_code::INVALID_FORMAT, "DATEADD second is out of range"))?;

    let date = NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|date| date.and_hms_opt(hour, minute, second))
        .ok_or_else(|| Failure::new(error_code::INVALID_FORMAT, "invalid DATEADD date/time"))?;
    let delta_seconds = [
        (days, 86_400_i64),
        (hours, 3_600_i64),
        (minutes, 60_i64),
        (seconds, 1_i64),
    ]
    .into_iter()
    .try_fold(0_i64, |total, (amount, scale)| {
        total.checked_add(amount.checked_mul(scale)?)
    })
    .ok_or_else(|| Failure::new(error_code::INVALID_FORMAT, "DATEADD offset is out of range"))?;
    let delta = DateDuration::try_seconds(delta_seconds).ok_or_else(|| {
        Failure::new(error_code::INVALID_FORMAT, "DATEADD offset is out of range")
    })?;
    let result = date.checked_add_signed(delta).ok_or_else(|| {
        Failure::new(error_code::INVALID_FORMAT, "DATEADD result is out of range")
    })?;

    Ok(format!(
        "[1,[{},{},{},{},{},{}]]",
        result.year(),
        result.month(),
        result.day(),
        result.hour(),
        result.minute(),
        result.second()
    ))
}

fn parse_integer_array<const N: usize>(input: &str, label: &str) -> Result<[i64; N], Failure> {
    let contents = input
        .strip_prefix('[')
        .and_then(|input| input.strip_suffix(']'))
        .ok_or_else(|| {
            Failure::new(
                error_code::INVALID_FORMAT,
                format!("{label} must be an array"),
            )
        })?;
    let values = contents
        .split(',')
        .map(|value| {
            value.trim().parse::<i64>().map_err(|_| {
                Failure::new(
                    error_code::INVALID_FORMAT,
                    format!("{label} contains a non-integer"),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    values.try_into().map_err(|values: Vec<i64>| {
        Failure::new(
            error_code::INVALID_FORMAT,
            format!("{label} needs {N} integers, got {}", values.len()),
        )
    })
}

fn time_with_offset(
    now: NaiveDateTime,
    offset: Option<&str>,
    label: &str,
) -> Result<String, Failure> {
    let Some(offset) = offset else {
        return Ok(format_time(now));
    };
    let delta_seconds = if let Ok(hours) = offset.parse::<i64>() {
        hours.checked_mul(3_600).ok_or_else(|| {
            Failure::new(
                error_code::INVALID_FORMAT,
                format!("{label} offset is out of range"),
            )
        })?
    } else {
        let [year, month, days, hours, minutes, seconds] =
            parse_integer_array::<6>(offset, &format!("{label} offset"))?;
        if year != 0 || month != 0 {
            return Err(Failure::new(
                error_code::INVALID_FORMAT,
                format!("{label} year and month offsets are unsupported"),
            ));
        }
        [
            (days, 86_400_i64),
            (hours, 3_600_i64),
            (minutes, 60_i64),
            (seconds, 1_i64),
        ]
        .into_iter()
        .try_fold(0_i64, |total, (amount, scale)| {
            total.checked_add(amount.checked_mul(scale)?)
        })
        .ok_or_else(|| {
            Failure::new(
                error_code::INVALID_FORMAT,
                format!("{label} offset is out of range"),
            )
        })?
    };
    let delta = DateDuration::try_seconds(delta_seconds).ok_or_else(|| {
        Failure::new(
            error_code::INVALID_FORMAT,
            format!("{label} offset is out of range"),
        )
    })?;
    let shifted = now.checked_add_signed(delta).ok_or_else(|| {
        Failure::new(
            error_code::INVALID_FORMAT,
            format!("{label} result is out of range"),
        )
    })?;
    Ok(format_time(shifted))
}

fn format_time(time: NaiveDateTime) -> String {
    format!(
        "[1,[{},{},{},{},{},{}]]",
        time.year(),
        time.month(),
        time.day(),
        time.hour(),
        time.minute(),
        time.second(),
    )
}

fn dated_log_directory(root: &Path, now: &DateTime<Local>) -> PathBuf {
    root.join("logs")
        .join(now.year().to_string())
        .join(now.month().to_string())
        .join(now.day().to_string())
}

fn main_log_path(root: &Path, now: &DateTime<Local>) -> PathBuf {
    dated_log_directory(root, now).join(format!(
        "{}-{}-{}.log",
        now.hour(),
        now.minute(),
        now.second()
    ))
}

fn log_filename(filename: &str) -> Result<String, String> {
    let filename = config::filename(filename)?;
    if Path::new(filename).extension().is_some() {
        Ok(filename.to_string())
    } else {
        Ok(format!("{filename}.log"))
    }
}

struct Coordinator {
    root: PathBuf,
    config: Snapshot,
    runtime_log: Arc<RuntimeLog>,
    lock: Lock,
    databases: HashMap<String, Pool>,
    protocols: HashMap<String, Protocol>,
    results: HashMap<u64, ResultState>,
    callback_notifications: VecDeque<u64>,
    reservations: ReservationCounter,
    next_id: u64,
    started: Instant,
    jobs: mpsc::Sender<Job>,
    close_events: mpsc::Sender<usize>,
    outstanding: usize,
    state: Lifecycle,
    #[cfg(feature = "profile-requests")]
    profiles: Option<profile::Session>,
}

enum Lifecycle {
    Running,
    Draining { reply: blocking::Sender<String> },
    Closing { reply: blocking::Sender<String> },
}

pub struct Engine {
    calls: mpsc::Sender<Call>,
    #[cfg(feature = "profile-requests")]
    profile_enabled: bool,
}
static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();

pub fn exchange(text: String, capacity: usize) -> String {
    if capacity < 32 {
        return String::new();
    }
    match ENGINE.get_or_init(Engine::start) {
        Ok(engine) => engine.exchange(text, capacity),
        Err(message) => error(message),
    }
}

impl Engine {
    fn start() -> Result<Self, String> {
        #[cfg(feature = "profile-requests")]
        let profiles = profile::init().map_err(|_| error_code::ERROR.to_string())?;
        #[cfg(feature = "profile-requests")]
        let profile_enabled = profiles.is_some();
        let root = config::root();
        let config = Snapshot::load(&root)?;
        let configured_workers = config.thread;
        let config_file = config.config_path().map(Path::to_path_buf);
        let (calls, receiver) = mpsc::channel(QUEUE);
        let (ready_tx, ready_rx) = blocking::channel();
        std::thread::Builder::new()
            .name("extdb3-runtime".into())
            .spawn(move || {
                let cpus = std::thread::available_parallelism().ok().map(usize::from);
                let worker_count = selected_workers(configured_workers, cpus);
                let now = Local::now();
                let runtime_log = Arc::new(RuntimeLog::new(&root, &now, config.log_flush()));
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(worker_count)
                    .enable_all()
                    .build();
                let Ok(runtime) = runtime else {
                    runtime_log.write_event("Failed to create Tokio multi-thread runtime");
                    let _ = ready_tx.send(false);
                    return;
                };
                let tokio_workers = runtime.metrics().num_workers();
                for message in startup_messages(
                    &root,
                    config_file.as_deref(),
                    configured_workers,
                    cpus,
                    tokio_workers,
                ) {
                    if let Err(error) = runtime_log.write_startup(&message) {
                        eprintln!("extDB3 Rust: could not write startup log: {error}");
                    }
                }
                let _ = ready_tx.send(true);
                runtime.block_on(async move {
                    let (jobs, queued) = mpsc::channel(QUEUE);
                    let (done_tx, done_rx) = mpsc::channel(COMPLETION_QUEUE);
                    let (close_tx, close_rx) = mpsc::channel(1);
                    tokio::spawn(workers(queued, done_tx));
                    Coordinator {
                        root,
                        config,
                        runtime_log: Arc::clone(&runtime_log),
                        lock: Lock::Open,
                        databases: HashMap::new(),
                        protocols: HashMap::new(),
                        results: HashMap::new(),
                        callback_notifications: VecDeque::new(),
                        reservations: ReservationCounter::default(),
                        next_id: 1,
                        started: Instant::now(),
                        jobs,
                        close_events: close_tx,
                        outstanding: 0,
                        state: Lifecycle::Running,
                        #[cfg(feature = "profile-requests")]
                        profiles,
                    }
                    .run(receiver, done_rx, close_rx)
                    .await;
                });
            })
            .map_err(|_| error_code::ERROR.to_string())?;
        if !ready_rx.recv().unwrap_or(false) {
            return Err(error_code::ERROR.into());
        }
        Ok(Self {
            calls,
            #[cfg(feature = "profile-requests")]
            profile_enabled,
        })
    }

    fn exchange(&self, text: String, capacity: usize) -> String {
        let (reply, response) = blocking::channel();
        if self
            .calls
            .try_send(Call {
                text,
                capacity,
                reply,
                #[cfg(feature = "profile-requests")]
                received: self.profile_enabled.then(Instant::now),
            })
            .is_err()
        {
            return error("Error Queue Full");
        }
        response.recv().unwrap_or_else(|_| error(error_code::ERROR))
    }
}

async fn workers(mut queued: mpsc::Receiver<Job>, done: mpsc::Sender<Completed>) {
    let mut active = JoinSet::<WorkerResult>::new();
    let mut tickets = HashMap::new();
    loop {
        tokio::select! {
            job = queued.recv(), if worker_has_capacity(active.len()) => {
                let Some(job) = job else { break; };
                let Job {
                    id,
                    protocol_type,
                    protocol,
                    query_name,
                    pool,
                    operation,
                    delivery,
                    runtime_log,
                    #[cfg(feature = "profile-requests")]
                    profile,
                } = job;
                #[cfg(feature = "profile-requests")]
                let profile_enabled = profile.is_some();
                let ticket = CompletionTicket {
                    id,
                    protocol_type,
                    protocol: protocol.clone(),
                    query_name: query_name.clone(),
                    delivery,
                    runtime_log: Arc::clone(&runtime_log),
                    #[cfg(feature = "profile-requests")]
                    profile,
                };
                let task = active.spawn(async move {
                    let context = format!(
                        "{protocol_type} request {id} protocol '{protocol}' call '{query_name}'",
                    );
                    #[cfg(feature = "profile-requests")]
                    let worker_started = profile_enabled.then(Instant::now);
                    #[cfg(feature = "profile-requests")]
                    let outcome = tokio::time::timeout(
                        Duration::from_secs(60),
                        async {
                            if profile_enabled {
                                let (result, execution) =
                                    sql_custom::execute_profiled(&pool, operation, MAX_RESULT)
                                        .await;
                                (result, Some(execution))
                            } else {
                                (sql_custom::execute(&pool, operation, MAX_RESULT).await, None)
                            }
                        },
                    )
                    .await;
                    #[cfg(feature = "profile-requests")]
                    let (result, execution) = match outcome {
                        Ok((Ok(value), execution)) => (value, execution),
                        Ok((Err(failure), execution)) => {
                            log_failure(&runtime_log, &context, &failure);
                            (error(failure.code), execution)
                        }
                        Err(_) => {
                            runtime_log.write_event(&format!("{context} failed: Error Query Timeout after 60 seconds"));
                            (
                                error("Error Query Timeout"),
                                profile_enabled.then(profile::Execution::default),
                            )
                        }
                    };
                    #[cfg(not(feature = "profile-requests"))]
                    let outcome = tokio::time::timeout(
                        Duration::from_secs(60),
                        sql_custom::execute(&pool, operation, MAX_RESULT),
                    ).await;
                    #[cfg(not(feature = "profile-requests"))]
                    let result = match outcome {
                        Ok(Ok(value)) => value,
                        Ok(Err(failure)) => {
                            log_failure(&runtime_log, &context, &failure);
                            error(failure.code)
                        }
                        Err(_) => {
                            runtime_log.write_event(&format!("{context} failed: Error Query Timeout after 60 seconds"));
                            error("Error Query Timeout")
                        }
                    };
                    drop(pool);
                    WorkerResult {
                        result,
                        #[cfg(feature = "profile-requests")]
                        worker_started,
                        #[cfg(feature = "profile-requests")]
                        worker_finished: worker_started.map(|_| Instant::now()),
                        #[cfg(feature = "profile-requests")]
                        execution,
                    }
                });
                tickets.insert(task.id(), ticket);
            }
            joined = active.join_next_with_id(), if !active.is_empty() => {
                let Some(joined) = joined else { continue; };
                match joined {
                    Ok((task_id, result)) => {
                        if let Some(ticket) = tickets.remove(&task_id) {
                            publish_completion(&done, ticket, result).await;
                        }
                    }
                    Err(join_error) => {
                        if let Some(ticket) = tickets.remove(&join_error.id()) {
                            let context = format!(
                                "{} request {} protocol '{}' call '{}'",
                                ticket.protocol_type, ticket.id, ticket.protocol, ticket.query_name,
                            );
                            ticket.runtime_log.write_event(&format!(
                                "{context} failed: execution {}",
                                if join_error.is_panic() { "panicked" } else { "was cancelled" }
                            ));
                            #[cfg(feature = "profile-requests")]
                            let result = failed_worker_result(ticket.profile.is_some());
                            #[cfg(not(feature = "profile-requests"))]
                            let result = failed_worker_result();
                            publish_completion(&done, ticket, result).await;
                        }
                    }
                }
            }
        }
    }
    while let Some(joined) = active.join_next_with_id().await {
        match joined {
            Ok((task_id, result)) => {
                if let Some(ticket) = tickets.remove(&task_id) {
                    publish_completion(&done, ticket, result).await;
                }
            }
            Err(join_error) => {
                if let Some(ticket) = tickets.remove(&join_error.id()) {
                    let context = format!(
                        "{} request {} protocol '{}' call '{}'",
                        ticket.protocol_type, ticket.id, ticket.protocol, ticket.query_name,
                    );
                    ticket.runtime_log.write_event(&format!(
                        "{context} failed: execution {}",
                        if join_error.is_panic() {
                            "panicked"
                        } else {
                            "was cancelled"
                        }
                    ));
                    #[cfg(feature = "profile-requests")]
                    let result = failed_worker_result(ticket.profile.is_some());
                    #[cfg(not(feature = "profile-requests"))]
                    let result = failed_worker_result();
                    publish_completion(&done, ticket, result).await;
                }
            }
        }
    }
}

fn worker_has_capacity(active_jobs: usize) -> bool {
    active_jobs < MAX_ACTIVE_JOBS
}

#[cfg(feature = "profile-requests")]
fn failed_worker_result(profiled: bool) -> WorkerResult {
    let started = profiled.then(Instant::now);
    WorkerResult {
        result: error(error_code::ERROR),
        worker_started: started,
        worker_finished: started.map(|_| Instant::now()),
        execution: profiled.then(profile::Execution::default),
    }
}

#[cfg(not(feature = "profile-requests"))]
fn failed_worker_result() -> WorkerResult {
    WorkerResult {
        result: error(error_code::ERROR),
    }
}

async fn publish_completion(
    done: &mpsc::Sender<Completed>,
    ticket: CompletionTicket,
    result: WorkerResult,
) {
    let _ = done
        .send(Completed {
            id: ticket.id,
            delivery: ticket.delivery,
            result: result.result,
            #[cfg(feature = "profile-requests")]
            profile: ticket.profile,
            #[cfg(feature = "profile-requests")]
            worker_started: result.worker_started,
            #[cfg(feature = "profile-requests")]
            worker_finished: result.worker_finished,
            #[cfg(feature = "profile-requests")]
            execution: result.execution,
        })
        .await;
}

fn log_failure(runtime_log: &RuntimeLog, context: &str, failure: &Failure) {
    runtime_log.write_event(&format!(
        "{context} failed: {}: {}",
        failure.code, failure.detail
    ));
}

impl Coordinator {
    async fn run(
        mut self,
        mut calls: mpsc::Receiver<Call>,
        mut completed: mpsc::Receiver<Completed>,
        mut closed: mpsc::Receiver<usize>,
    ) {
        let mut callback_retry = tokio::time::interval(CALLBACK_RETRY_INTERVAL);
        callback_retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                Some(done) = completed.recv() => self.complete(done),
                Some(call) = calls.recv(), if matches!(&self.state, Lifecycle::Running) => self.call(call).await,
                Some(failures) = closed.recv(), if matches!(&self.state, Lifecycle::Closing { .. }) => self.finish_reset(failures),
                _ = callback_retry.tick(), if !self.callback_notifications.is_empty() => self.flush_callback_notifications(),
                else => break,
            }
        }
    }

    fn complete(&mut self, done: Completed) {
        #[cfg(feature = "profile-requests")]
        if let (
            Some(profiles),
            Some(request),
            Some(worker_started),
            Some(worker_finished),
            Some(execution),
        ) = (
            self.profiles.as_mut(),
            done.profile,
            done.worker_started,
            done.worker_finished,
            done.execution,
        ) {
            profiles.record_request(
                request,
                worker_started,
                worker_finished,
                Instant::now(),
                execution,
                &done.result,
            );
        }
        debug_assert!(self.outstanding > 0);
        self.outstanding = self.outstanding.saturating_sub(1);
        match done.delivery {
            Delivery::Discard => {}
            Delivery::Saved(reservation) => {
                self.results.insert(
                    done.id,
                    ResultState::Ready {
                        text: done.result,
                        offset: 0,
                        _reservation: reservation,
                    },
                );
            }
            Delivery::Callback(reservation) => {
                self.results.insert(
                    done.id,
                    ResultState::Ready {
                        text: done.result,
                        offset: 0,
                        _reservation: reservation,
                    },
                );
                #[cfg(feature = "profile-requests")]
                if let Some(profiles) = self.profiles.as_mut() {
                    profiles.callback_result_ready(done.id, Instant::now());
                }
                self.callback_notifications.push_back(done.id);
            }
            Delivery::Sync {
                reply,
                capacity,
                reservation,
            } => {
                if done.result.len() <= capacity {
                    drop(reservation);
                    let _ = reply.send(done.result);
                } else {
                    self.results.insert(
                        done.id,
                        ResultState::Ready {
                            text: done.result,
                            offset: 0,
                            _reservation: reservation,
                        },
                    );
                    if reply.send(format!("[2,\"{}\"]", done.id)).is_err() {
                        self.remove_result(done.id);
                    }
                }
            }
        }
        if self.outstanding == 0 && matches!(&self.state, Lifecycle::Draining { .. }) {
            self.start_closing();
        }
    }

    fn start_closing(&mut self) {
        let Lifecycle::Draining { reply } = std::mem::replace(&mut self.state, Lifecycle::Running)
        else {
            return;
        };
        self.protocols.clear();
        let pools = std::mem::take(&mut self.databases);
        self.state = Lifecycle::Closing { reply };
        let closed = self.close_events.clone();
        tokio::spawn(async move {
            let mut failures = 0;
            for pool in pools.into_values() {
                if pool.disconnect().await.is_err() {
                    failures += 1;
                }
            }
            let _ = closed.send(failures).await;
        });
    }

    fn finish_reset(&mut self, failures: usize) {
        let Lifecycle::Closing { reply } = std::mem::replace(&mut self.state, Lifecycle::Running)
        else {
            return;
        };
        if failures == 0 {
            let _ = reply.send("[1]".into());
        } else {
            self.runtime_log.write_event(&format!(
                "RESET completed with {failures} database pool close failure(s)"
            ));
            let _ = reply.send(error(error_code::ERROR));
        }
    }

    fn begin_reset(&mut self, reply: &blocking::Sender<String>) -> Result<(), Failure> {
        if !self.config.allow_reset {
            return Err(Failure::new(
                error_code::ERROR,
                "RESET is disabled in [Main]",
            ));
        }
        if !matches!(&self.lock, Lock::Open) {
            return Err(Failure::new(
                error_code::INVALID_FORMAT,
                "RESET requires an unlocked extension",
            ));
        }
        self.state = Lifecycle::Draining {
            reply: reply.clone(),
        };
        if self.outstanding == 0 {
            self.start_closing();
        }
        Ok(())
    }

    fn remove_result(&mut self, id: u64) {
        self.results.remove(&id);
        self.callback_notifications.retain(|pending| *pending != id);
        #[cfg(feature = "profile-requests")]
        if let Some(profiles) = self.profiles.as_mut() {
            profiles.discard_callback(id);
        }
    }

    fn flush_callback_notifications(&mut self) {
        while let Some(id) = self.callback_notifications.pop_front() {
            if !matches!(self.results.get(&id), Some(ResultState::Ready { .. })) {
                #[cfg(feature = "profile-requests")]
                if let Some(profiles) = self.profiles.as_mut() {
                    profiles.discard_callback(id);
                }
                continue;
            }
            #[cfg(feature = "profile-requests")]
            let started = self.profiles.as_ref().map(|_| Instant::now());
            let outcome = callback::notify_result_ready(id);
            #[cfg(feature = "profile-requests")]
            if let (Some(profiles), Some(started)) = (self.profiles.as_mut(), started) {
                let attempt = match outcome {
                    Ok(_) => profile::CallbackAttempt::Accepted,
                    Err(callback::NotifyError::QueueFull) => profile::CallbackAttempt::QueueFull,
                    Err(callback::NotifyError::Unavailable) => {
                        profile::CallbackAttempt::Unavailable
                    }
                };
                profiles.callback_attempt(id, started, Instant::now(), attempt);
            }
            match outcome {
                Ok(slots_left) if slots_left > 0 => {}
                Ok(_) => return,
                Err(_) => {
                    self.callback_notifications.push_front(id);
                    return;
                }
            }
        }
    }

    async fn call(&mut self, call: Call) {
        #[cfg(feature = "profile-requests")]
        let coordinator_started = self.profiles.as_ref().map(|_| Instant::now());
        let (kind, tail) = match call.text.split_once(':') {
            Some(parts) => parts,
            None => {
                let _ = call.reply.send(error(error_code::INVALID_MESSAGE));
                return;
            }
        };
        match kind {
            "9" => {
                if tail == "RESET" {
                    match self.begin_reset(&call.reply) {
                        Ok(()) => return,
                        Err(failure) => {
                            log_failure(&self.runtime_log, "System command 'RESET'", &failure);
                            let _ = call.reply.send(error(failure.code));
                            return;
                        }
                    }
                }
                let value = match self.system(tail, call.capacity).await {
                    Ok(value) => value,
                    Err(failure) => {
                        let command = system_command_label(tail);
                        log_failure(
                            &self.runtime_log,
                            &format!("System command '{command}'"),
                            &failure,
                        );
                        error(failure.code)
                    }
                };
                let _ = call.reply.send(value);
            }
            "4" | "5" => {
                let value = tail
                    .parse::<u64>()
                    .ok()
                    .map(|id| self.poll(id, kind == "5", call.capacity))
                    .unwrap_or_default();
                let _ = call.reply.send(value);
            }
            "0" | "1" | "2" | "6" => {
                if kind == "6" && !callback::is_registered() {
                    let _ = call.reply.send(error("Error Callback Unavailable"));
                    return;
                }
                let Some((name, payload)) = tail.split_once(':') else {
                    let _ = call.reply.send(error(error_code::INVALID_FORMAT));
                    return;
                };
                let Some(protocol) = self.protocols.get(name) else {
                    self.runtime_log.write_event(&format!(
                        "Error Unknown Protocol: {name}; Input String omitted"
                    ));
                    let _ = call.reply.send(if kind == "1" {
                        String::new()
                    } else {
                        error(error_code::UNKNOWN_PROTOCOL)
                    });
                    return;
                };
                if let Protocol::Log(filename) = protocol {
                    let result = self.runtime_log.write(filename.as_deref(), payload);
                    if let Err(ref detail) = result {
                        self.runtime_log
                            .write_event(&format!("LOG: Error: {detail}"));
                    }
                    let _ = call.reply.send(if kind == "1" {
                        String::new()
                    } else if result.is_ok() {
                        "[1]".into()
                    } else {
                        error(error_code::LOGGER)
                    });
                    return;
                }
                let Protocol::Sql { pool, definition } = protocol else {
                    unreachable!()
                };
                let protocol_type = match definition {
                    sql_custom::ProtocolDefinition::Raw(_) => "SQL",
                    sql_custom::ProtocolDefinition::Custom(_) => "SQL_CUSTOM",
                };
                let pool = pool.clone();
                let (query_name, operation) = match sql_custom::bind(definition, payload) {
                    Ok(bound) => bound,
                    Err(failure) => {
                        if failure.code == error_code::NO_CUSTOM_CALL {
                            self.runtime_log
                                .write_event(&format!("SQL_CUSTOM: {}", failure.detail));
                        } else {
                            log_failure(
                                &self.runtime_log,
                                &format!("{protocol_type} protocol '{name}' input"),
                                &failure,
                            );
                        }
                        let _ = call.reply.send(if kind == "1" {
                            String::new()
                        } else {
                            error(failure.code)
                        });
                        return;
                    }
                };
                let mut reservation = if kind == "1" {
                    None
                } else {
                    match self.reservations.try_reserve() {
                        Some(reservation) => Some(reservation),
                        None => {
                            let _ = call.reply.send(error("Error Result Limit"));
                            return;
                        }
                    }
                };
                let Some(next) = self.next_id.checked_add(1) else {
                    let _ = call.reply.send(error("Error ID Limit"));
                    return;
                };
                let id = self.next_id;
                let delivery = match kind {
                    "0" => Delivery::Sync {
                        reply: call.reply.clone(),
                        capacity: call.capacity,
                        reservation: reservation
                            .take()
                            .expect("synchronous SQL calls reserve a result slot"),
                    },
                    "1" => Delivery::Discard,
                    "6" => Delivery::Callback(
                        reservation
                            .take()
                            .expect("callback SQL calls reserve a result slot"),
                    ),
                    _ => Delivery::Saved(
                        reservation
                            .take()
                            .expect("asynchronous SQL calls reserve a result slot"),
                    ),
                };
                let job = Job {
                    id,
                    protocol_type,
                    protocol: name.to_string(),
                    query_name: query_name.to_string(),
                    pool,
                    operation,
                    delivery,
                    runtime_log: Arc::clone(&self.runtime_log),
                    #[cfg(feature = "profile-requests")]
                    profile: self.profiles.as_ref().map(|_| profile::Request {
                        id,
                        kind: kind.to_string(),
                        query: query_name.to_string(),
                        received: call
                            .received
                            .expect("profile-enabled calls capture their receive time"),
                        coordinator: coordinator_started
                            .expect("profile-enabled calls capture coordinator time"),
                        enqueued: Instant::now(),
                    }),
                };
                let Some(outstanding) = self.outstanding.checked_add(1) else {
                    let _ = call.reply.send(error(error_code::ERROR));
                    return;
                };
                if self.jobs.try_send(job).is_err() {
                    let _ = call.reply.send(error("Error Queue Full"));
                    return;
                }
                self.outstanding = outstanding;
                self.next_id = next;
                if kind == "2" || kind == "6" {
                    self.results.insert(id, ResultState::Pending);
                }
                if kind == "1" {
                    let _ = call.reply.send(String::new());
                } else if kind == "2" || kind == "6" {
                    let _ = call.reply.send(format!("[2,\"{id}\"]"));
                }
            }
            _ => {
                let _ = call.reply.send(error(error_code::INVALID_MESSAGE));
            }
        }
    }

    fn poll(&mut self, id: u64, multipart: bool, capacity: usize) -> String {
        let Some(state) = self.results.get_mut(&id) else {
            return String::new();
        };
        let ResultState::Ready { text, offset, .. } = state else {
            return "[3]".into();
        };
        if !multipart {
            if *offset > 0 || text.len() > capacity {
                return "[5]".into();
            }
            let value = text.clone();
            self.remove_result(id);
            return value;
        }
        if *offset == text.len() {
            self.remove_result(id);
            return String::new();
        }
        multipart_chunk(text, offset, capacity)
    }

    async fn system(&mut self, command: &str, capacity: usize) -> Result<String, Failure> {
        let fields: Vec<_> = command.split(':').collect();
        match fields.as_slice() {
            ["VERSION"] => return Ok("1.033".into()),
            ["LOCK_STATUS"] => {
                return Ok(if matches!(self.lock, Lock::Open) {
                    "[0]"
                } else {
                    "[1]"
                }
                .into());
            }
            ["UNLOCK", code] => {
                let correct = matches!(&self.lock, Lock::Code(expected) if expected == code);
                if correct {
                    self.lock = Lock::Open;
                }
                return Ok(if correct { "[1]" } else { "[0]" }.into());
            }
            ["LOCAL_TIME"] => {
                return time_with_offset(Local::now().naive_local(), None, "LOCAL_TIME");
            }
            ["LOCAL_TIME", offset] => {
                return time_with_offset(Local::now().naive_local(), Some(offset), "LOCAL_TIME");
            }
            ["UTC_TIME"] => {
                return time_with_offset(chrono::Utc::now().naive_utc(), None, "UTC_TIME");
            }
            ["UTC_TIME", offset] => {
                return time_with_offset(chrono::Utc::now().naive_utc(), Some(offset), "UTC_TIME");
            }
            ["UPTIME", unit] => {
                let seconds = self.started.elapsed().as_secs();
                return Ok(match *unit {
                    "SECONDS" => seconds,
                    "MINUTES" => seconds / 60,
                    "HOURS" => seconds / 3600,
                    _ => {
                        return Err(Failure::new(
                            error_code::INVALID_FORMAT,
                            format!("unsupported UPTIME unit '{unit}'"),
                        ));
                    }
                }
                .to_string());
            }
            ["DATEADD", date, offset] => return date_add(date, offset),
            _ => {}
        }
        if !matches!(self.lock, Lock::Open) {
            return Err(Failure::new(
                error_code::INVALID_FORMAT,
                "system command is unavailable while the extension is locked",
            ));
        }
        match fields.as_slice() {
            ["OUTPUTSIZE"] => {
                self.runtime_log
                    .write_legacy_event(&format!("Output Size: {capacity}"));
                Ok(capacity.to_string())
            }
            ["LOCK"] => {
                self.lock = Lock::Permanent;
                self.runtime_log.write_legacy_event("Locked");
                Ok("[1]".into())
            }
            ["LOCK", code] => {
                self.lock = Lock::Code(code.to_string());
                self.runtime_log
                    .write_legacy_event("Locked (unlock code configured)");
                Ok("[1]".into())
            }
            ["ADD_DATABASE", section] => self.add_database(section, section).await,
            ["ADD_DATABASE", section, alias] => self.add_database(section, alias).await,
            ["ADD_DATABASE_PROTOCOL", database, "SQL", name] => {
                self.add_raw_sql_protocol(database, name, "")
            }
            ["ADD_DATABASE_PROTOCOL", database, "SQL", name, options] => {
                self.add_raw_sql_protocol(database, name, options)
            }
            [
                "ADD_DATABASE_PROTOCOL",
                database,
                "SQL_CUSTOM",
                name,
                filename,
            ] => {
                if self.protocols.contains_key(*name) || self.protocols.len() >= 64 {
                    return Err(Failure::new(
                        error_code::PROTOCOL_NAME_TAKEN,
                        format!("protocol name '{name}' is already in use"),
                    ));
                }
                let pool = self
                    .databases
                    .get(*database)
                    .ok_or_else(|| {
                        Failure::new(
                            error_code::DATABASE_CONFIG,
                            format!("unknown database alias '{database}'"),
                        )
                    })?
                    .clone();
                let path = self.root.join("sql_custom").join(
                    config::filename(filename)
                        .map_err(|detail| Failure::new(error_code::INVALID_FORMAT, detail))?,
                );
                let custom = match Custom::load(&path) {
                    Ok(custom) => custom,
                    Err(detail) => {
                        return Err(Failure::new(
                            error_code::FAILED_TO_LOAD_PROTOCOL,
                            format!(
                                "SQL_CUSTOM protocol '{name}' from {}: {detail}",
                                path.display()
                            ),
                        ));
                    }
                };
                self.protocols.insert(
                    name.to_string(),
                    Protocol::Sql {
                        pool,
                        definition: sql_custom::ProtocolDefinition::Custom(Arc::new(custom)),
                    },
                );
                Ok("[1]".into())
            }
            ["ADD_PROTOCOL", "LOG", name] => {
                if self.protocols.contains_key(*name) || self.protocols.len() >= 64 {
                    return Err(Failure::new(
                        error_code::PROTOCOL_NAME_TAKEN,
                        format!("protocol name '{name}' is already in use"),
                    ));
                }
                self.runtime_log.register(None).map_err(|detail| {
                    Failure::new(
                        error_code::FAILED_TO_LOAD_PROTOCOL,
                        format!("LOG protocol '{name}': {detail}"),
                    )
                })?;
                self.protocols.insert(name.to_string(), Protocol::Log(None));
                Ok("[1]".into())
            }
            ["ADD_PROTOCOL", "LOG", name, filename] => {
                if self.protocols.contains_key(*name) || self.protocols.len() >= 64 {
                    return Err(Failure::new(
                        error_code::PROTOCOL_NAME_TAKEN,
                        format!("protocol name '{name}' is already in use"),
                    ));
                }
                let filename = log_filename(filename)
                    .map_err(|detail| Failure::new(error_code::INVALID_FORMAT, detail))?;
                self.runtime_log
                    .register(Some(&filename))
                    .map_err(|detail| {
                        Failure::new(
                            error_code::FAILED_TO_LOAD_PROTOCOL,
                            format!("LOG protocol '{name}' ({filename}): {detail}"),
                        )
                    })?;
                self.protocols
                    .insert(name.to_string(), Protocol::Log(Some(filename)));
                Ok("[1]".into())
            }
            _ => Err(Failure::new(
                error_code::INVALID_FORMAT,
                format!(
                    "unsupported system command '{}'",
                    system_command_label(command)
                ),
            )),
        }
    }

    fn add_raw_sql_protocol(
        &mut self,
        database: &str,
        name: &str,
        options: &str,
    ) -> Result<String, Failure> {
        if self.protocols.contains_key(name) || self.protocols.len() >= 64 {
            return Err(Failure::new(
                error_code::PROTOCOL_NAME_TAKEN,
                format!("protocol name '{name}' is already in use"),
            ));
        }
        let pool = self
            .databases
            .get(database)
            .ok_or_else(|| {
                Failure::new(
                    error_code::DATABASE_CONFIG,
                    format!("unknown database alias '{database}'"),
                )
            })?
            .clone();
        let options = sql_custom::RawOptions::parse(options)
            .map_err(|detail| Failure::new(error_code::INVALID_FORMAT, detail))?;
        self.protocols.insert(
            name.to_owned(),
            Protocol::Sql {
                pool,
                definition: sql_custom::ProtocolDefinition::Raw(options),
            },
        );
        Ok("[1]".into())
    }

    async fn add_database(&mut self, section: &str, alias: &str) -> Result<String, Failure> {
        if self.databases.contains_key(alias) || self.databases.len() >= 16 {
            return Err(Failure::new(
                error_code::ALREADY_CONNECTED_TO_DATABASE,
                format!("database alias '{alias}' is already connected"),
            ));
        }
        let options = self
            .config
            .database(&self.root, section)
            .map_err(|detail| {
                let code = if detail == error_code::TLS_CONFIG {
                    error_code::TLS_CONFIG
                } else {
                    error_code::DATABASE_CONFIG
                };
                Failure::new(code, format!("database section '{section}': {detail}"))
            })?;
        let pool = Pool::new(options);
        let connected = tokio::time::timeout(Duration::from_secs(10), pool.get_conn()).await;
        match connected {
            Ok(Ok(connection)) => {
                drop(connection);
                self.databases.insert(alias.into(), pool);
                self.runtime_log.write_event(&format!(
                    "Connected to Database: section '{section}', alias '{alias}'"
                ));
                Ok("[1]".into())
            }
            Ok(Err(detail)) => {
                let _ = pool.disconnect().await;
                Err(Failure::new(
                    error_code::DATABASE_CONNECTION,
                    format!("SQL: No Database Connection: section '{section}': {detail}"),
                ))
            }
            Err(_) => {
                let _ = pool.disconnect().await;
                Err(Failure::new(
                    error_code::DATABASE_CONNECTION,
                    format!(
                        "SQL: No Database Connection: section '{section}': timed out after 10 seconds"
                    ),
                ))
            }
        }
    }
}

fn multipart_chunk(text: &str, offset: &mut usize, capacity: usize) -> String {
    let mut end = (*offset + capacity).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let chunk = text[*offset..end].to_string();
    *offset = end;
    chunk
}

#[cfg(feature = "synthetic-bench")]
pub fn multipart_bytes_for_benchmark(text: &str, capacity: usize) -> usize {
    if capacity == 0 {
        return 0;
    }
    let mut offset = 0;
    let mut copied = 0;
    while offset < text.len() {
        copied += multipart_chunk(text, &mut offset, capacity).len();
    }
    copied
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEMPORARY_DIRECTORY_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    struct TemporaryDirectory(PathBuf);

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temporary_root() -> TemporaryDirectory {
        for _ in 0..32 {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = TEMPORARY_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "exile-extdb3-engine-test-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return TemporaryDirectory(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("could not create test directory: {error}"),
            }
        }
        panic!("could not create a unique test directory after 32 attempts");
    }

    fn coordinator_at(root: &Path) -> Coordinator {
        let (jobs, _) = mpsc::channel(1);
        let (close_events, _) = mpsc::channel(1);
        let now = Local::now();
        Coordinator {
            root: root.to_path_buf(),
            config: Snapshot::load(root).unwrap(),
            runtime_log: Arc::new(RuntimeLog::new(root, &now, true)),
            lock: Lock::Open,
            databases: HashMap::new(),
            protocols: HashMap::new(),
            results: HashMap::new(),
            callback_notifications: VecDeque::new(),
            reservations: ReservationCounter::default(),
            next_id: 1,
            started: Instant::now(),
            jobs,
            close_events,
            outstanding: 0,
            state: Lifecycle::Running,
            #[cfg(feature = "profile-requests")]
            profiles: None,
        }
    }

    fn coordinator() -> Coordinator {
        coordinator_at(&PathBuf::new())
    }

    #[test]
    fn temporary_roots_are_unique() {
        let first = temporary_root();
        let second = temporary_root();
        assert_ne!(first.0, second.0);
    }

    fn log_call(text: &str) -> (Call, blocking::Receiver<String>) {
        let (reply, response) = blocking::channel();
        (
            Call {
                text: text.to_string(),
                capacity: 1024,
                reply,
                #[cfg(feature = "profile-requests")]
                received: None,
            },
            response,
        )
    }

    fn completed(id: u64, delivery: Delivery, result: &str) -> Completed {
        #[cfg(feature = "profile-requests")]
        let now = Instant::now();
        Completed {
            id,
            delivery,
            result: result.to_string(),
            #[cfg(feature = "profile-requests")]
            profile: Some(profile::Request {
                id,
                kind: "0".into(),
                query: "test".into(),
                received: now,
                coordinator: now,
                enqueued: now,
            }),
            #[cfg(feature = "profile-requests")]
            worker_started: Some(now),
            #[cfg(feature = "profile-requests")]
            worker_finished: Some(now),
            #[cfg(feature = "profile-requests")]
            execution: Some(profile::Execution::default()),
        }
    }

    #[test]
    fn reports_extdb3_1033_protocol_version() {
        let mut c = coordinator();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        assert_eq!(runtime.block_on(c.system("VERSION", 32)).unwrap(), "1.033");
    }

    #[test]
    fn startup_message_reports_runtime_platform_cpu_threads_and_configuration() {
        let root = temporary_root();
        let messages = startup_messages(&root.0, None, 6, Some(16), 6);
        assert!(
            messages
                .iter()
                .any(|line| line.contains("Unable to find extdb3-conf.ini"))
        );

        fs::write(root.0.join("extdb3-conf.ini"), "[Main]\n").unwrap();
        let config_file = root.0.join("extdb3-conf.ini");
        let messages = startup_messages(&root.0, Some(&config_file), 6, Some(16), 6);
        let platform = match std::env::consts::OS {
            "linux" => "Linux",
            "windows" => "Windows",
            "macos" => "macOS",
            other => other,
        };

        assert!(messages.iter().any(|line| line.contains("Version: 0.1.0")));
        assert!(messages.iter().any(|line| line == "extDB3: Version: 1.033"));
        assert!(
            messages
                .iter()
                .any(|line| line == &format!("extDB3: {platform} Version"))
        );
        assert!(
            messages.iter().any(
                |line| line.contains("extDB3: Detected 16 Cores, Setting up 6 Worker Threads")
            )
        );
        assert!(messages.iter().any(|line| {
            line.contains("Using the new Rust runtime: Tokio multi-thread scheduler")
        }));
        assert!(
            messages
                .iter()
                .any(|line| line.contains("Found extdb3-conf.ini"))
        );
        assert!(messages.iter().any(|line| {
            line.contains("Thread setting: 6, detected logical CPUs: 16, selected Tokio workers: 6")
        }));
        assert_eq!(selected_workers(0, Some(16)), 16);
        assert_eq!(selected_workers(3, Some(16)), 3);
    }

    #[tokio::test]
    async fn worker_leaves_queued_job_until_an_active_slot_opens() {
        let mut active = JoinSet::<()>::new();
        let (release, wait) = tokio::sync::oneshot::channel();
        active.spawn(async move {
            let _ = wait.await;
        });
        for _ in 1..MAX_ACTIVE_JOBS {
            active.spawn(std::future::pending::<()>());
        }

        let (queued, mut waiting) = mpsc::channel(1);
        queued.send(42).await.unwrap();

        assert_eq!(active.len(), MAX_ACTIVE_JOBS);
        assert!(!worker_has_capacity(active.len()));
        assert_eq!(waiting.len(), 1);

        release.send(()).unwrap();
        active.join_next().await.unwrap().unwrap();

        assert!(worker_has_capacity(active.len()));
        assert_eq!(waiting.try_recv().unwrap(), 42);
        active.abort_all();
        while active.join_next().await.is_some() {}
    }

    #[test]
    fn log_line_matches_extdb3_timestamp_and_numeric_thread_format() {
        let now = Local::now();
        let line = format_log_line(&now, 42, "runtime detail");
        let (timestamp, rest) = line.split_once("] [Thread ").unwrap();
        assert!(timestamp.starts_with('['));
        assert!(rest.starts_with("42] runtime detail"));

        let clock = timestamp.trim_start_matches('[');
        let time = clock.split_once(' ').unwrap().0;
        #[cfg(windows)]
        {
            assert_eq!(time.split(':').count(), 4);
            let micros = time.rsplit(':').next().unwrap();
            assert_eq!(micros.len(), 6);
            assert!(micros.chars().all(|digit| digit.is_ascii_digit()));
        }
        #[cfg(not(windows))]
        assert_eq!(time.split(':').count(), 3);
    }

    #[test]
    fn startup_log_lines_keep_the_original_unprefixed_format() {
        let root = temporary_root();
        let now = Local::now();
        let main_path = main_log_path(&root.0, &now);
        let runtime_log = RuntimeLog::new(&root.0, &now, true);
        let startup_message = startup_messages(&root.0, None, 0, Some(8), 8)
            .into_iter()
            .find(|message| message == "extDB3: Version: 1.033")
            .unwrap();

        runtime_log.write_startup(&startup_message).unwrap();
        runtime_log
            .write_at(None, "extDB3 Rust: runtime detail", &now)
            .unwrap();

        let lines = fs::read_to_string(main_path).unwrap();
        let mut lines = lines.lines();
        assert_eq!(lines.next(), Some(startup_message.as_str()));
        assert!(lines.next().unwrap().contains("[Thread "));
    }

    #[test]
    fn system_command_log_label_excludes_argument_text() {
        assert_eq!(
            system_command_label("ADD_DATABASE:exile:password"),
            "ADD_DATABASE"
        );
        assert_eq!(system_command_label("LOCK:secret"), "LOCK");
        assert_eq!(system_command_label("bad command:secret"), "<invalid>");
    }

    #[test]
    fn dateadd_matches_extdb3_array_contract_and_calendar_rollover() {
        assert_eq!(
            date_add("[2024,2,28,23,30,0]", "[1,1,0,0]").unwrap(),
            "[1,[2024,3,1,0,30,0]]"
        );
        assert_eq!(
            date_add("[2025,1,1,0,0,0]", "[-1,0,0,0]").unwrap(),
            "[1,[2024,12,31,0,0,0]]"
        );
    }

    #[test]
    fn dateadd_stays_available_after_lock_and_logs_invalid_input_detail() {
        let root = temporary_root();
        let now = Local::now();
        let main_path = main_log_path(&root.0, &now);
        let mut c = coordinator_at(&root.0);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        assert_eq!(runtime.block_on(c.system("LOCK", 1024)).unwrap(), "[1]");
        assert_eq!(
            runtime
                .block_on(c.system("DATEADD:[2024,2,28,23,30,0]:[1,1,0,0]", 1024))
                .unwrap(),
            "[1,[2024,3,1,0,30,0]]"
        );

        let (call, response) = log_call("9:DATEADD:[2024]:[1,0,0,0]");
        runtime.block_on(c.call(call));
        assert_eq!(response.recv().unwrap(), "[0,\"Error Invalid Format\"]");
        let log = fs::read_to_string(main_path).unwrap();
        assert!(log.contains("System command 'DATEADD' failed: Error Invalid Format"));
        assert!(log.contains("DATEADD date needs 6 integers, got 1"));
    }

    #[test]
    fn output_size_and_lock_events_are_written_to_the_main_log() {
        let root = temporary_root();
        let now = Local::now();
        let main_path = main_log_path(&root.0, &now);
        let mut c = coordinator_at(&root.0);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        assert_eq!(
            runtime.block_on(c.system("OUTPUTSIZE", 20479)).unwrap(),
            "20479"
        );
        assert_eq!(runtime.block_on(c.system("LOCK", 20479)).unwrap(), "[1]");

        let log = fs::read_to_string(main_path).unwrap();
        assert!(log.contains("] [Thread ") && log.contains("extDB3: Output Size: 20479"));
        assert!(log.contains("extDB3: Locked"));
    }

    #[test]
    fn execution_failure_log_keeps_details_out_of_the_sqf_error_text() {
        let root = temporary_root();
        let now = Local::now();
        let main_path = main_log_path(&root.0, &now);
        let runtime_log = RuntimeLog::new(&root.0, &now, true);
        let failure = Failure::new(
            error_code::MARIADB_QUERY,
            "server returned duplicate key error",
        );

        log_failure(
            &runtime_log,
            "SQL_CUSTOM request 42 call 'savePlayer'",
            &failure,
        );

        assert_eq!(
            error(failure.code),
            "[0,\"Error MariaDBQueryException Exception\"]"
        );
        let log = fs::read_to_string(main_path).unwrap();
        assert!(log.contains("SQL_CUSTOM request 42 call 'savePlayer'"));
        assert!(log.contains("Error MariaDBQueryException Exception"));
        assert!(log.contains("server returned duplicate key error"));
    }

    #[test]
    fn log_protocol_writes_custom_and_default_logs_in_extdb3_date_folders() {
        let root = temporary_root();
        let now = Local::now();
        let main_path = main_log_path(&root.0, &now);
        let custom_path = dated_log_directory(&root.0, &now).join("Exile_TradingLog.log");
        let mut c = coordinator_at(&root.0);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        assert_eq!(
            runtime
                .block_on(c.system("ADD_PROTOCOL:LOG:TRADING:Exile_TradingLog", 1024))
                .unwrap(),
            "[1]"
        );
        assert!(custom_path.is_file(), "LOG registration opens its file");
        assert_eq!(
            runtime
                .block_on(c.system("ADD_PROTOCOL:LOG:MAIN", 1024))
                .unwrap(),
            "[1]"
        );

        let (call, response) = log_call("0:TRADING:trade:player:76561198000000000");
        runtime.block_on(c.call(call));
        assert_eq!(response.recv().unwrap(), "[1]");

        let (call, response) = log_call("1:MAIN:runtime message");
        runtime.block_on(c.call(call));
        assert_eq!(response.recv().unwrap(), "");

        let later = now + DateDuration::days(2);
        c.runtime_log
            .write_at(Some("Exile_TradingLog.log"), "later message", &later)
            .unwrap();

        let custom_log = fs::read_to_string(custom_path).unwrap();
        let main_log = fs::read_to_string(main_path).unwrap();
        assert!(custom_log.contains(" [Thread "));
        assert!(custom_log.ends_with("] later message\n"));
        assert!(custom_log.contains("trade:player:76561198000000000"));
        assert!(
            !dated_log_directory(&root.0, &later)
                .join("Exile_TradingLog.log")
                .exists()
        );
        assert!(main_log.contains(" [Thread "));
        assert!(main_log.ends_with("] runtime message\n"));
        assert_eq!(
            dated_log_directory(&root.0, &now),
            root.0
                .join("logs")
                .join(now.year().to_string())
                .join(now.month().to_string())
                .join(now.day().to_string())
        );
    }

    #[test]
    fn rotating_log_keeps_the_active_file_and_three_backups() {
        let root = temporary_root();
        let path = root.0.join("rotating.log");
        let mut log = RotatingFile::open(path.clone(), true, 4, 3).unwrap();

        for line in [
            b"1\n".as_slice(),
            b"2\n",
            b"3\n",
            b"4\n",
            b"5\n",
            b"6\n",
            b"7\n",
        ] {
            log.write_line(line).unwrap();
        }

        assert_eq!(fs::read_to_string(&path).unwrap(), "7\n");
        assert_eq!(fs::read_to_string(backup_path(&path, 1)).unwrap(), "5\n6\n");
        assert_eq!(fs::read_to_string(backup_path(&path, 2)).unwrap(), "3\n4\n");
        assert_eq!(fs::read_to_string(backup_path(&path, 3)).unwrap(), "1\n2\n");
    }
    #[test]
    fn polling_consumes_only_complete_results() {
        let mut c = coordinator();
        let reservation = c.reservations.try_reserve().unwrap();
        c.results.insert(1, ResultState::Pending);
        assert_eq!(c.poll(1, false, 32), "[3]");
        c.results.insert(
            1,
            ResultState::Ready {
                text: "[1,[[\"hello\"]]]".into(),
                offset: 0,
                _reservation: reservation,
            },
        );
        assert_eq!(c.poll(1, false, 3), "[5]");
        let mut assembled = String::new();
        loop {
            let fragment = c.poll(1, true, 4);
            if fragment.is_empty() {
                break;
            }
            assembled.push_str(&fragment);
        }
        assert_eq!(assembled, "[1,[[\"hello\"]]]");
        assert!(!c.results.contains_key(&1));
        assert_eq!(c.poll(1, false, 32), "");
    }

    #[test]
    fn callback_async_results_remain_pollable_and_queue_the_request_id() {
        let mut c = coordinator();
        c.outstanding = 1;
        let reservation = c.reservations.try_reserve().unwrap();
        c.complete(completed(
            42,
            Delivery::Callback(reservation),
            "[1,[[\"ready\"]]]",
        ));

        assert_eq!(c.callback_notifications, VecDeque::from([42]));
        assert_eq!(c.poll(42, false, 64), "[1,[[\"ready\"]]]");
        assert!(c.results.is_empty());
        assert!(c.callback_notifications.is_empty());
        c.flush_callback_notifications();
        assert!(c.callback_notifications.is_empty());
    }

    #[test]
    fn callback_unavailable_keeps_the_notification_for_retry() {
        let mut c = coordinator();
        c.results.insert(
            42,
            ResultState::Ready {
                text: "[1]".into(),
                offset: 0,
                _reservation: c.reservations.try_reserve().unwrap(),
            },
        );
        c.callback_notifications.push_back(42);

        c.flush_callback_notifications();

        assert_eq!(c.callback_notifications, VecDeque::from([42]));
    }

    #[test]
    fn oversized_sync_response_keeps_its_slot_until_multipart_polling_finishes() {
        let mut c = coordinator();
        c.outstanding = 1;
        let reservation = c.reservations.try_reserve().unwrap();
        let (reply, response) = blocking::channel();
        c.complete(completed(
            7,
            Delivery::Sync {
                reply,
                capacity: 3,
                reservation,
            },
            "[1,[[\"hello\"]]]",
        ));
        assert_eq!(response.recv().unwrap(), "[2,\"7\"]");

        let outstanding = (0..RESULTS - 1)
            .map(|_| c.reservations.try_reserve().unwrap())
            .collect::<Vec<_>>();
        assert!(c.reservations.try_reserve().is_none());
        drop(outstanding);

        let mut assembled = String::new();
        loop {
            let fragment = c.poll(7, true, 4);
            if fragment.is_empty() {
                break;
            }
            assembled.push_str(&fragment);
        }
        assert_eq!(assembled, "[1,[[\"hello\"]]]");
        let available = (0..RESULTS)
            .map(|_| c.reservations.try_reserve().unwrap())
            .collect::<Vec<_>>();
        assert!(c.reservations.try_reserve().is_none());
        drop(available);
    }

    #[test]
    fn result_reservation_limit_is_released_on_drop() {
        let reservations = ReservationCounter::default();
        let mut outstanding = (0..RESULTS)
            .map(|_| reservations.try_reserve().unwrap())
            .collect::<Vec<_>>();
        assert!(reservations.try_reserve().is_none());
        drop(outstanding.pop());
        let replacement = reservations.try_reserve().unwrap();
        assert!(reservations.try_reserve().is_none());
        drop(replacement);
    }

    #[test]
    fn unicode_fragments_preserve_bytes() {
        let mut c = coordinator();
        c.results.insert(
            7,
            ResultState::Ready {
                text: "a€b".into(),
                offset: 0,
                _reservation: c.reservations.try_reserve().unwrap(),
            },
        );
        assert_eq!(c.poll(7, true, 3), "a");
        assert_eq!(c.poll(7, true, 3), "€");
        assert_eq!(c.poll(7, true, 3), "b");
        assert!(c.results.contains_key(&7));
        assert_eq!(c.poll(7, true, 3), "");
        assert!(!c.results.contains_key(&7));
    }
}
