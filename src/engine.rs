use crate::{
    config,
    sql_custom::{self, Custom, Query},
};
use chrono::{Datelike, Timelike};
use mysql_async::Pool;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, OnceLock, mpsc as blocking},
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinSet};

const QUEUE: usize = 256;
const COMPLETION_QUEUE: usize = 16;
const RESULTS: usize = 128;
pub const MAX_INPUT: usize = 256 * 1024;
const MAX_RESULT: usize = 1024 * 1024;

pub fn error(message: &str) -> String {
    format!("[0,\"{}\"]", message.replace('"', "\"\""))
}

struct Call {
    text: String,
    capacity: usize,
    reply: blocking::Sender<String>,
}
struct Job {
    id: u64,
    pool: Pool,
    query: Arc<Query>,
    inputs: Vec<Vec<mysql_async::Value>>,
    delivery: Delivery,
}
enum Delivery {
    Discard,
    Saved,
    Sync {
        reply: blocking::Sender<String>,
        capacity: usize,
    },
}
struct Completed {
    id: u64,
    delivery: Delivery,
    result: String,
}
enum ResultState {
    Pending,
    Ready { text: String, offset: usize },
}
enum Lock {
    Open,
    Permanent,
    Code(String),
}
enum Protocol {
    Sql { pool: Pool, custom: Arc<Custom> },
    Log(PathBuf),
}

struct Coordinator {
    root: PathBuf,
    lock: Lock,
    databases: HashMap<String, Pool>,
    protocols: HashMap<String, Protocol>,
    results: HashMap<u64, ResultState>,
    reservations: usize,
    next_id: u64,
    started: Instant,
    jobs: mpsc::Sender<Job>,
}

pub struct Engine {
    calls: mpsc::Sender<Call>,
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
        let (calls, receiver) = mpsc::channel(QUEUE);
        let (ready_tx, ready_rx) = blocking::channel();
        std::thread::Builder::new()
            .name("extdb3-runtime".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build();
                let Ok(runtime) = runtime else {
                    let _ = ready_tx.send(false);
                    return;
                };
                let _ = ready_tx.send(true);
                runtime.block_on(async move {
                    let (jobs, queued) = mpsc::channel(QUEUE);
                    let (done_tx, done_rx) = mpsc::channel(COMPLETION_QUEUE);
                    tokio::spawn(workers(queued, done_tx));
                    Coordinator {
                        root: config::root(),
                        lock: Lock::Open,
                        databases: HashMap::new(),
                        protocols: HashMap::new(),
                        results: HashMap::new(),
                        reservations: 0,
                        next_id: 1,
                        started: Instant::now(),
                        jobs,
                    }
                    .run(receiver, done_rx)
                    .await;
                });
            })
            .map_err(|_| "Error Runtime".to_string())?;
        if !ready_rx.recv().unwrap_or(false) {
            return Err("Error Runtime".into());
        }
        Ok(Self { calls })
    }

    fn exchange(&self, text: String, capacity: usize) -> String {
        let (reply, response) = blocking::channel();
        if self
            .calls
            .try_send(Call {
                text,
                capacity,
                reply,
            })
            .is_err()
        {
            return error("Error Queue Full");
        }
        response.recv().unwrap_or_else(|_| error("Error Runtime"))
    }
}

async fn workers(mut queued: mpsc::Receiver<Job>, done: mpsc::Sender<Completed>) {
    let mut active = JoinSet::new();
    loop {
        tokio::select! {
            job = queued.recv() => {
                let Some(job) = job else { break; };
                let done = done.clone();
                active.spawn(async move {
                    let outcome = tokio::time::timeout(Duration::from_secs(60),
                        sql_custom::execute(&job.pool, &job.query, job.inputs, MAX_RESULT)).await;
                    let result = match outcome {
                        Ok(Ok(value)) => value,
                        Ok(Err(message)) => error(&message),
                        Err(_) => error("Error Query Timeout"),
                    };
                    let _ = done.send(Completed { id: job.id, delivery: job.delivery, result }).await;
                });
            }
            _ = active.join_next(), if !active.is_empty() => {}
        }
    }
    while active.join_next().await.is_some() {}
}

impl Coordinator {
    async fn run(
        mut self,
        mut calls: mpsc::Receiver<Call>,
        mut completed: mpsc::Receiver<Completed>,
    ) {
        loop {
            tokio::select! {
                Some(done) = completed.recv() => self.complete(done),
                Some(call) = calls.recv() => self.call(call).await,
                else => break,
            }
        }
    }

    fn complete(&mut self, done: Completed) {
        match done.delivery {
            Delivery::Discard => {}
            Delivery::Saved => {
                self.results.insert(
                    done.id,
                    ResultState::Ready {
                        text: done.result,
                        offset: 0,
                    },
                );
            }
            Delivery::Sync { reply, capacity } => {
                if done.result.len() <= capacity {
                    self.remove_result(done.id);
                    let _ = reply.send(done.result);
                } else {
                    self.results.insert(
                        done.id,
                        ResultState::Ready {
                            text: done.result,
                            offset: 0,
                        },
                    );
                    if reply.send(format!("[2,\"{}\"]", done.id)).is_err() {
                        self.remove_result(done.id);
                    }
                }
            }
        }
    }

    fn remove_result(&mut self, id: u64) {
        if self.results.remove(&id).is_some() {
            self.reservations = self.reservations.saturating_sub(1);
        }
    }

    async fn call(&mut self, call: Call) {
        let (kind, tail) = match call.text.split_once(':') {
            Some(parts) => parts,
            None => {
                let _ = call.reply.send(error("Error Invalid Message"));
                return;
            }
        };
        match kind {
            "9" => {
                let value = self
                    .system(tail, call.capacity)
                    .await
                    .unwrap_or_else(|e| error(&e));
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
            "0" | "1" | "2" => {
                let Some((name, payload)) = tail.split_once(':') else {
                    let _ = call.reply.send(error("Error Invalid Format"));
                    return;
                };
                let Some(protocol) = self.protocols.get(name) else {
                    let _ = call.reply.send(if kind == "1" {
                        String::new()
                    } else {
                        error("Error Unknown Protocol")
                    });
                    return;
                };
                if let Protocol::Log(path) = protocol {
                    use std::io::Write;
                    let result = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .and_then(|mut file| writeln!(file, "{payload}"));
                    let _ = call.reply.send(if kind == "1" {
                        String::new()
                    } else if result.is_ok() {
                        "[1]".into()
                    } else {
                        error("Error Log")
                    });
                    return;
                }
                let Protocol::Sql { pool, custom } = protocol else {
                    unreachable!()
                };
                let (query_name, data) = payload.split_once(':').unwrap_or((payload, ""));
                let Some(query) = custom.queries.get(query_name) else {
                    let _ = call.reply.send(if kind == "1" {
                        String::new()
                    } else {
                        error("Error No Custom Call Not Found")
                    });
                    return;
                };
                let inputs = match query.bind(data) {
                    Ok(v) => v,
                    Err(_) => {
                        let _ = call.reply.send(if kind == "1" {
                            String::new()
                        } else {
                            error("Error Invalid Inputs")
                        });
                        return;
                    }
                };
                let tracked = kind != "1";
                if tracked && self.reservations >= RESULTS {
                    let _ = call.reply.send(error("Error Result Limit"));
                    return;
                }
                let Some(next) = self.next_id.checked_add(1) else {
                    let _ = call.reply.send(error("Error ID Limit"));
                    return;
                };
                let id = self.next_id;
                let delivery = match kind {
                    "0" => Delivery::Sync {
                        reply: call.reply.clone(),
                        capacity: call.capacity,
                    },
                    "1" => Delivery::Discard,
                    _ => Delivery::Saved,
                };
                let job = Job {
                    id,
                    pool: pool.clone(),
                    query: query.clone(),
                    inputs,
                    delivery,
                };
                if self.jobs.try_send(job).is_err() {
                    let _ = call.reply.send(error("Error Queue Full"));
                    return;
                }
                self.next_id = next;
                if tracked {
                    self.reservations += 1;
                    self.results.insert(id, ResultState::Pending);
                }
                if kind == "1" {
                    let _ = call.reply.send(String::new());
                } else if kind == "2" {
                    let _ = call.reply.send(format!("[2,\"{id}\"]"));
                }
            }
            _ => {
                let _ = call.reply.send(error("Error Invalid Message"));
            }
        }
    }

    fn poll(&mut self, id: u64, multipart: bool, capacity: usize) -> String {
        let Some(state) = self.results.get_mut(&id) else {
            return String::new();
        };
        let ResultState::Ready { text, offset } = state else {
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

    async fn system(&mut self, command: &str, capacity: usize) -> Result<String, String> {
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
                let t = chrono::Local::now();
                return Ok(format!(
                    "[1,[{},{},{},{},{},{}]]",
                    t.year(),
                    t.month(),
                    t.day(),
                    t.hour(),
                    t.minute(),
                    t.second()
                ));
            }
            ["UTC_TIME"] => {
                let t = chrono::Utc::now();
                return Ok(format!(
                    "[1,[{},{},{},{},{},{}]]",
                    t.year(),
                    t.month(),
                    t.day(),
                    t.hour(),
                    t.minute(),
                    t.second()
                ));
            }
            ["UPTIME", unit] => {
                let seconds = self.started.elapsed().as_secs();
                return Ok(match *unit {
                    "SECONDS" => seconds,
                    "MINUTES" => seconds / 60,
                    "HOURS" => seconds / 3600,
                    _ => return Err("Error Invalid Format".into()),
                }
                .to_string());
            }
            _ => {}
        }
        if !matches!(self.lock, Lock::Open) {
            return Err("Error Invalid Format".into());
        }
        match fields.as_slice() {
            ["OUTPUTSIZE"] => Ok(capacity.to_string()),
            ["LOCK"] => {
                self.lock = Lock::Permanent;
                Ok("[1]".into())
            }
            ["LOCK", code] => {
                self.lock = Lock::Code(code.to_string());
                Ok("[1]".into())
            }
            ["ADD_DATABASE", section] => self.add_database(section, section).await,
            ["ADD_DATABASE", section, alias] => self.add_database(section, alias).await,
            [
                "ADD_DATABASE_PROTOCOL",
                database,
                "SQL_CUSTOM",
                name,
                filename,
            ] => {
                if self.protocols.contains_key(*name) || self.protocols.len() >= 64 {
                    return Err("Error Protocol Already Exists".into());
                }
                let pool = self
                    .databases
                    .get(*database)
                    .ok_or("Error Unknown Database")?
                    .clone();
                let path = self
                    .root
                    .join("sql_custom")
                    .join(config::filename(filename)?);
                let custom =
                    Custom::load(&path).map_err(|_| "Failed to Load Protocol".to_string())?;
                self.protocols.insert(
                    name.to_string(),
                    Protocol::Sql {
                        pool,
                        custom: Arc::new(custom),
                    },
                );
                Ok("[1]".into())
            }
            ["ADD_PROTOCOL", "LOG", name, filename] => {
                if self.protocols.contains_key(*name) || self.protocols.len() >= 64 {
                    return Err("Error Protocol Already Exists".into());
                }
                let dir = self.root.join("logs");
                std::fs::create_dir_all(&dir).map_err(|_| "Error Log".to_string())?;
                let path = dir.join(format!("{}.log", config::filename(filename)?));
                self.protocols.insert(name.to_string(), Protocol::Log(path));
                Ok("[1]".into())
            }
            _ => Err("Error Invalid Format".into()),
        }
    }

    async fn add_database(&mut self, section: &str, alias: &str) -> Result<String, String> {
        if self.databases.contains_key(alias) || self.databases.len() >= 16 {
            return Err("Error Database Already Exists".into());
        }
        let pool = Pool::new(config::database(&self.root, section)?);
        let connected = tokio::time::timeout(Duration::from_secs(10), pool.get_conn()).await;
        match connected {
            Ok(Ok(connection)) => {
                drop(connection);
                self.databases.insert(alias.into(), pool);
                Ok("[1]".into())
            }
            _ => {
                let _ = pool.disconnect().await;
                Err("Database Config Error".into())
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
    fn coordinator() -> Coordinator {
        let (jobs, _) = mpsc::channel(1);
        Coordinator {
            root: PathBuf::new(),
            lock: Lock::Open,
            databases: HashMap::new(),
            protocols: HashMap::new(),
            results: HashMap::new(),
            reservations: 0,
            next_id: 1,
            started: Instant::now(),
            jobs,
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
    fn polling_consumes_only_complete_results() {
        let mut c = coordinator();
        c.results.insert(1, ResultState::Pending);
        assert_eq!(c.poll(1, false, 32), "[3]");
        c.results.insert(
            1,
            ResultState::Ready {
                text: "[1,[[\"hello\"]]]".into(),
                offset: 0,
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
    fn unicode_fragments_preserve_bytes() {
        let mut c = coordinator();
        c.results.insert(
            7,
            ResultState::Ready {
                text: "a€b".into(),
                offset: 0,
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
