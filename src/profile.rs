use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

static OUTPUT: OnceLock<Mutex<File>> = OnceLock::new();

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

pub fn init() -> Result<(), std::io::Error> {
    let Some(path) = std::env::var_os("EXTDB3_PROFILE_PATH").map(PathBuf::from) else {
        return Ok(());
    };
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    writeln!(
        file,
        "request_id,kind,query,call_queue_us,prepare_us,job_queue_us,pool_acquire_us,mysql_async_us,format_us,worker_us,completion_queue_us,end_to_end_us,rows,response_bytes,outcome"
    )?;
    OUTPUT
        .set(Mutex::new(file))
        .map_err(|_| std::io::Error::other("profile output is already initialized"))
}

pub fn record(
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
    if let Some(output) = OUTPUT.get() {
        let mut output = output
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = output.write_all(row.as_bytes());
    }
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

    #[test]
    fn csv_fields_quote_delimiters_and_quotes() {
        assert_eq!(csv_field("query"), "query");
        assert_eq!(csv_field("a,\"b"), "\"a,\"\"b\"");
    }
}
