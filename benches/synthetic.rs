use extDB3_x64::synthetic_bench::{
    Query, Writer, format_rows, literal, multipart_bytes, parse_custom,
};
use mysql_async::Value;
use std::{fs, hint::black_box, path::PathBuf, sync::Arc, time::Instant};

const SAMPLES: usize = 7;
const TARGET_BYTES_PER_SAMPLE: usize = 4 * 1024 * 1024;
const MIN_ITERATIONS: usize = 5;
const MAX_ITERATIONS: usize = 200_000;

struct Point {
    series: &'static str,
    scale: usize,
    work_bytes: usize,
    iterations: usize,
    median_ns: f64,
    min_ns: f64,
    max_ns: f64,
}

fn measure(
    series: &'static str,
    scale: usize,
    work_bytes: usize,
    mut operation: impl FnMut() -> usize,
) -> Point {
    let iterations =
        (TARGET_BYTES_PER_SAMPLE / work_bytes.max(1)).clamp(MIN_ITERATIONS, MAX_ITERATIONS);
    let mut samples = Vec::with_capacity(SAMPLES);

    for sample in 0..=SAMPLES {
        let started = Instant::now();
        for _ in 0..iterations {
            black_box(operation());
        }
        let elapsed = started.elapsed();
        if sample > 0 {
            samples.push(elapsed.as_nanos() as f64 / iterations as f64);
        }
    }

    samples.sort_by(f64::total_cmp);
    Point {
        series,
        scale,
        work_bytes,
        iterations,
        median_ns: samples[SAMPLES / 2],
        min_ns: samples[0],
        max_ns: samples[SAMPLES - 1],
    }
}

fn custom_config(strip: &str, input_count: usize) -> String {
    let mut config = format!("[Default]\nVersion=1\nStrip Chars={strip}\n[q]\nSQL1_1=SELECT ?\n");
    if input_count > 0 {
        config.push_str("SQL1_INPUTS=");
        for index in 1..=input_count {
            if index > 1 {
                config.push(',');
            }
            config.push_str(&index.to_string());
        }
        config.push('\n');
    }
    config
}

fn parse_config(query_count: usize) -> String {
    let mut config = String::from("[Default]\nVersion=1\n");
    for index in 0..query_count {
        config.push_str(&format!("[q{index}]\nSQL1_1=SELECT 1\n"));
    }
    config
}

fn ascii_strip_chars(count: usize) -> String {
    (33u8..=126)
        .filter(|value| *value != b'z' && *value != b'"')
        .take(count)
        .map(char::from)
        .collect()
}

fn result_query() -> Arc<Query> {
    let custom = parse_custom(
        "[Default]\nVersion=1\nStrip Chars=\";\"\n[q]\nSQL1_1=SELECT 1\nOUTPUT=2-STRING,3-STRING\n",
    )
    .unwrap();
    custom.queries["q"].clone()
}

fn result_rows(count: usize) -> Vec<Vec<Value>> {
    let row = vec![
        Value::UInt(42),
        Value::Bytes(b"player-1234".to_vec()),
        Value::Bytes(b"text with a \"quoted\" value".to_vec()),
        Value::Double(123.456789),
        Value::Bytes(b"true".to_vec()),
    ];
    vec![row; count]
}

fn interleaved_sizes(sizes: &[usize]) -> Vec<usize> {
    let mut remaining = sizes.iter();
    let mut ordered = Vec::with_capacity(sizes.len());
    loop {
        if let Some(size) = remaining.next() {
            ordered.push(*size);
        }
        if let Some(size) = remaining.next_back() {
            ordered.push(*size);
        }
        if remaining.len() == 0 {
            return ordered;
        }
    }
}

fn all_points() -> Vec<Point> {
    let mut points = Vec::new();

    for size in interleaved_sizes(&[16, 64, 256, 1_024, 4_096, 16_384, 65_536]) {
        let literal_text = format!("\"{}\"", "x".repeat(size - 2));
        points.push(measure("sqf_literal_bytes", size, size, || {
            usize::from(literal(black_box(&literal_text)))
        }));

        let quoted_text = "x\"".repeat(size / 2);
        points.push(measure("sqf_quote_bytes", size, size, || {
            let mut out = Writer::new(size * 2 + 2);
            out.quoted(black_box(&quoted_text)).unwrap();
            out.finish().len()
        }));
    }

    for size in interleaved_sizes(&[1, 4, 16, 64, 256]) {
        let config = custom_config("\";\"", size);
        let custom = parse_custom(&config).unwrap();
        let query = &custom.queries["q"];
        let data = (0..size)
            .map(|_| "alpha;beta")
            .collect::<Vec<_>>()
            .join(":");
        points.push(measure(
            "sql_custom_bind_arguments",
            size,
            data.len(),
            || {
                query
                    .bind(black_box(&data))
                    .unwrap()
                    .iter()
                    .flatten()
                    .count()
            },
        ));
    }

    for size in interleaved_sizes(&[64, 1_024, 16_384, 65_536, 262_144]) {
        let data = "z".repeat(size);
        let config = custom_config("\"!\"", 1);
        let custom = parse_custom(&config).unwrap();
        let query = &custom.queries["q"];
        points.push(measure("sql_custom_bind_input_bytes", size, size, || {
            query
                .bind(black_box(&data))
                .unwrap()
                .iter()
                .flatten()
                .map(|value| match value {
                    Value::Bytes(bytes) => bytes.len(),
                    _ => 0,
                })
                .sum()
        }));
    }

    for strip_size in interleaved_sizes(&[1, 8, 32, 64]) {
        let strip = ascii_strip_chars(strip_size);
        let config = custom_config(&strip, 1);
        let custom = parse_custom(&config).unwrap();
        let query = &custom.queries["q"];
        let data = "z".repeat(4_096);
        points.push(measure(
            "sql_custom_bind_strip_chars",
            strip_size,
            data.len(),
            || {
                query
                    .bind(black_box(&data))
                    .unwrap()
                    .iter()
                    .flatten()
                    .count()
            },
        ));
    }

    for size in interleaved_sizes(&[1, 4, 16, 64, 256, 1_024]) {
        let config = parse_config(size);
        points.push(measure(
            "sql_custom_parse_queries",
            size,
            config.len(),
            || parse_custom(black_box(&config)).unwrap().queries.len(),
        ));
    }

    let query = result_query();
    for size in interleaved_sizes(&[1, 4, 16, 64, 256, 1_024, 4_096]) {
        let rows = result_rows(size);
        let work_bytes = format_rows(&query, &rows, 123, 16 * 1024 * 1024)
            .unwrap()
            .len();
        points.push(measure("format_result_rows", size, work_bytes, || {
            format_rows(black_box(&query), black_box(&rows), 123, 16 * 1024 * 1024)
                .unwrap()
                .len()
        }));
    }

    for size in interleaved_sizes(&[4_096, 16_384, 65_536, 262_144, 1_048_576]) {
        let text = "x".repeat(size);
        points.push(measure("multipart_result_bytes", size, size, || {
            multipart_bytes(black_box(&text), 4_096)
        }));
    }

    points
}

fn exponent(points: &[&Point]) -> Option<f64> {
    if points.len() < 2 {
        return None;
    }
    let log_x = points
        .iter()
        .map(|point| (point.scale as f64).ln())
        .collect::<Vec<_>>();
    let log_y = points
        .iter()
        .map(|point| point.median_ns.ln())
        .collect::<Vec<_>>();
    let mean_x = log_x.iter().sum::<f64>() / log_x.len() as f64;
    let mean_y = log_y.iter().sum::<f64>() / log_y.len() as f64;
    let covariance = log_x
        .iter()
        .zip(&log_y)
        .map(|(x, y)| (x - mean_x) * (y - mean_y))
        .sum::<f64>();
    let variance = log_x.iter().map(|x| (x - mean_x).powi(2)).sum::<f64>();
    (variance > 0.0).then_some(covariance / variance)
}

fn main() {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".audit/synthetic-benchmark.csv"));
    let mut points = all_points();
    points.sort_by(|left, right| {
        left.series
            .cmp(right.series)
            .then(left.scale.cmp(&right.scale))
    });
    let mut csv = String::from(
        "series,scale,work_bytes,operations_per_sample,median_ns,min_ns,max_ns,mib_per_second,observed_alpha\n",
    );

    for point in &points {
        let group = points
            .iter()
            .filter(|candidate| candidate.series == point.series);
        let group = group.collect::<Vec<_>>();
        let alpha = exponent(&group)
            .map(|value| format!("{value:.3}"))
            .unwrap_or_default();
        let mib_per_second =
            point.work_bytes as f64 * 1_000_000_000.0 / point.median_ns / (1024.0 * 1024.0);
        csv.push_str(&format!(
            "{},{},{},{},{:.1},{:.1},{:.1},{:.2},{}\n",
            point.series,
            point.scale,
            point.work_bytes,
            point.iterations,
            point.median_ns,
            point.min_ns,
            point.max_ns,
            mib_per_second,
            alpha
        ));
    }

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).expect("create benchmark output directory");
    }
    fs::write(&output, csv).expect("write benchmark CSV");
    eprintln!("synthetic benchmark results: {}", output.display());
}
