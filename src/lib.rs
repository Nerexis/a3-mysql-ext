#![deny(unsafe_op_in_unsafe_fn)]
#![allow(non_snake_case)]

mod config;
mod engine;
mod ffi;
mod sqf;
mod sql_custom;

#[cfg(feature = "synthetic-bench")]
pub mod synthetic_bench {
    pub use crate::sqf::{Writer, literal, text};
    pub use crate::sql_custom::{Custom, Query};

    use mysql_async::Value;

    pub fn parse_custom(text: &str) -> Result<Custom, String> {
        crate::sql_custom::parse_for_benchmark(text)
    }

    pub fn format_rows(
        query: &Query,
        rows: &[Vec<Value>],
        insert_id: u64,
        max_bytes: usize,
    ) -> Result<String, String> {
        crate::sql_custom::format_rows_for_benchmark(query, rows, insert_id, max_bytes)
    }

    pub fn multipart_bytes(text: &str, capacity: usize) -> usize {
        crate::engine::multipart_bytes_for_benchmark(text, capacity)
    }
}
