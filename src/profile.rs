use std::{
    collections::HashMap,
    ffi::OsString,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub struct Request {
    pub id: u64,
    pub kind: String,
    pub query: String,
    pub received: Instant,
    pub coordinator: Instant,
    pub enqueued: Instant,
}

#[derive(Default)]
pub struct Execution {
    pub pool_acquire_us: u64,
    pub mysql_async_us: u64,
    pub format_us: u64,
    pub rows: usize,
}

pub struct Session {
    sql_output: File,
    callback_output: File,
    callback_requests: HashMap<u64, CallbackRequest>,
}

struct CallbackRequest {
    result_ready: Instant,
    first_attempt: Option<Instant>,
    attempts: u64,
    queue_full_responses: u64,
    invocation_us: u64,
    accepted: bool,
}

#[derive(Clone, Copy)]
pub enum CallbackAttempt {
    Accepted,
    QueueFull,
    Unavailable,
}

impl Session {
    pub fn open(path: &Path) -> Result<Self, std::io::Error> {
        let mut sql_output = open_output(path)?;
        writeln!(
            sql_output,
            "request_id,kind,query,call_queue_us,prepare_us,job_queue_us,pool_acquire_us,mysql_async_us,format_us,worker_us,completion_queue_us,end_to_end_us,rows,response_bytes,outcome"
        )?;

        let mut callback_path = OsString::from(path.as_os_str());
        callback_path.push(".callbacks.csv");
        let mut callback_output = open_output(Path::new(&callback_path))?;
        writeln!(
            callback_output,
            "request_id,ready_to_first_notify_us,ready_to_accepted_notify_us,attempts,queue_full_responses,callback_invocation_us,accepted"
        )?;

        Ok(Self {
            sql_output,
            callback_output,
            callback_requests: HashMap::new(),
        })
    }

    pub fn record_request(
        &mut self,
        request: Request,
        worker_started: Instant,
        worker_finished: Instant,
        completed: Instant,
        execution: Execution,
        response: &str,
    ) {
        let call_queue = request.coordinator.duration_since(request.received);
        let prepare = request.enqueued.duration_since(request.coordinator);
        let job_queue = worker_started.duration_since(request.enqueued);
        let worker = worker_finished.duration_since(worker_started);
        let completion_queue = completed.duration_since(worker_finished);
        let end_to_end = completed.duration_since(request.received);
        let outcome = if response.starts_with("[1") {
            "ok"
        } else {
            "error"
        };
        let row = format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            request.id,
            csv_field(&request.kind),
            csv_field(&request.query),
            micros(call_queue),
            micros(prepare),
            micros(job_queue),
            execution.pool_acquire_us,
            execution.mysql_async_us,
            execution.format_us,
            micros(worker),
            micros(completion_queue),
            micros(end_to_end),
            execution.rows,
            response.len(),
            outcome,
        );
        let _ = self.sql_output.write_all(row.as_bytes());
    }

    pub fn callback_result_ready(&mut self, id: u64, result_ready: Instant) {
        self.callback_requests.entry(id).or_insert(CallbackRequest {
            result_ready,
            first_attempt: None,
            attempts: 0,
            queue_full_responses: 0,
            invocation_us: 0,
            accepted: false,
        });
    }

    pub fn callback_attempt(
        &mut self,
        id: u64,
        started: Instant,
        finished: Instant,
        outcome: CallbackAttempt,
    ) {
        let Some(request) = self.callback_requests.get_mut(&id) else {
            return;
        };
        request.first_attempt.get_or_insert(started);
        request.attempts = request.attempts.saturating_add(1);
        request.invocation_us = request
            .invocation_us
            .saturating_add(micros(finished.duration_since(started)));

        if matches!(outcome, CallbackAttempt::QueueFull) {
            request.queue_full_responses = request.queue_full_responses.saturating_add(1);
        }
        if !matches!(outcome, CallbackAttempt::Accepted) {
            return;
        }

        request.accepted = true;
        let ready_to_first = request
            .first_attempt
            .expect("accepted notifications have an attempt")
            .duration_since(request.result_ready);
        let ready_to_accepted = finished.duration_since(request.result_ready);
        let row = format!(
            "{id},{},{},{},{},{},{}\n",
            micros(ready_to_first),
            micros(ready_to_accepted),
            request.attempts,
            request.queue_full_responses,
            request.invocation_us,
            u8::from(request.accepted),
        );
        let _ = self.callback_output.write_all(row.as_bytes());
        self.callback_requests.remove(&id);
    }

    pub fn discard_callback(&mut self, id: u64) {
        self.callback_requests.remove(&id);
    }
}

pub fn init() -> Result<Option<Session>, std::io::Error> {
    init_from_path(std::env::var_os("EXTDB3_PROFILE_PATH").map(PathBuf::from))
}

fn init_from_path(path: Option<PathBuf>) -> Result<Option<Session>, std::io::Error> {
    path.as_deref().map(Session::open).transpose()
}

fn open_output(path: &Path) -> Result<File, std::io::Error> {
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
}

fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

fn csv_field(value: &str) -> String {
    if value
        .chars()
        .any(|character| matches!(character, ',' | '"' | '\n' | '\r'))
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TemporaryDirectory(PathBuf);

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temporary_directory() -> TemporaryDirectory {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "exile-extdb3-profile-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        TemporaryDirectory(path)
    }

    #[test]
    fn accepted_callback_writes_only_numeric_notification_metrics() {
        let directory = temporary_directory();
        let path = directory.0.join("startup.csv");
        let mut session = Session::open(&path).unwrap();
        let ready = Instant::now();
        let first = ready + Duration::from_micros(100);
        let rejected = first + Duration::from_micros(10);
        let accepted = rejected + Duration::from_micros(20);

        session.callback_result_ready(42, ready);
        session.callback_attempt(42, first, rejected, CallbackAttempt::QueueFull);
        session.callback_attempt(
            42,
            accepted,
            accepted + Duration::from_micros(5),
            CallbackAttempt::Accepted,
        );
        drop(session);

        let callback_csv =
            fs::read_to_string(directory.0.join("startup.csv.callbacks.csv")).unwrap();
        let lines = callback_csv.lines().collect::<Vec<_>>();
        assert_eq!(
            lines[0],
            "request_id,ready_to_first_notify_us,ready_to_accepted_notify_us,attempts,queue_full_responses,callback_invocation_us,accepted"
        );
        assert_eq!(lines[1], "42,100,135,2,1,15,1");
        assert_eq!(lines.len(), 2);
        assert!(!lines[1].chars().any(char::is_alphabetic));
    }

    #[test]
    fn nonaccepted_callback_attempts_do_not_write_rows() {
        let directory = temporary_directory();
        let path = directory.0.join("startup.csv");
        let mut session = Session::open(&path).unwrap();
        let ready = Instant::now();
        session.callback_result_ready(7, ready);
        session.callback_attempt(
            7,
            ready,
            ready + Duration::from_micros(4),
            CallbackAttempt::Unavailable,
        );
        drop(session);

        let callback_csv =
            fs::read_to_string(directory.0.join("startup.csv.callbacks.csv")).unwrap();
        assert_eq!(callback_csv.lines().count(), 1);
    }

    #[test]
    fn disabled_profile_path_creates_no_profile_session() {
        assert!(super::init_from_path(None).unwrap().is_none());
    }

    #[test]
    fn sql_profile_keeps_its_existing_header_and_row_fields() {
        let directory = temporary_directory();
        let path = directory.0.join("startup.csv");
        let mut session = Session::open(&path).unwrap();
        let received = Instant::now();
        let coordinator = received + Duration::from_micros(10);
        let enqueued = coordinator + Duration::from_micros(20);
        let worker_started = enqueued + Duration::from_micros(30);
        let worker_finished = worker_started + Duration::from_micros(70);
        let completed = worker_finished + Duration::from_micros(80);

        session.record_request(
            Request {
                id: 17,
                kind: "6".into(),
                query: "savePlayer".into(),
                received,
                coordinator,
                enqueued,
            },
            worker_started,
            worker_finished,
            completed,
            Execution {
                pool_acquire_us: 40,
                mysql_async_us: 50,
                format_us: 60,
                rows: 3,
            },
            "[1,[]]",
        );
        drop(session);

        let sql_csv = fs::read_to_string(path).unwrap();
        let lines = sql_csv.lines().collect::<Vec<_>>();
        assert_eq!(
            lines[0],
            "request_id,kind,query,call_queue_us,prepare_us,job_queue_us,pool_acquire_us,mysql_async_us,format_us,worker_us,completion_queue_us,end_to_end_us,rows,response_bytes,outcome"
        );
        assert_eq!(
            lines[1],
            "17,6,savePlayer,10,20,30,40,50,60,70,80,210,3,6,ok"
        );
    }

    #[test]
    fn csv_fields_quote_delimiters_and_quotes() {
        assert_eq!(csv_field("query"), "query");
        assert_eq!(csv_field("a,\"b"), "\"a,\"\"b\"");
    }
}
