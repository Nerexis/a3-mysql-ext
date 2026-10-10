#[cfg(feature = "profile-requests")]
use crate::profile::Execution as ProfileExecution;
use crate::sqf;
use mysql_async::{Params, Pool, Value, prelude::Queryable};
#[cfg(feature = "profile-requests")]
use std::time::{Duration, Instant};
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    sync::Arc,
};

pub struct Custom {
    pub queries: HashMap<String, Arc<Query>>,
}
pub struct Query {
    statements: Vec<Statement>,
    output: HashMap<usize, Vec<Filter>>,
    strip: StripChars,
    strip_mode: StripMode,
    insert_id: bool,
    insert_string: bool,
}
struct Statement {
    sql: String,
    inputs: Vec<Mapping>,
}
struct Mapping {
    index: usize,
    filters: Vec<Filter>,
}
#[derive(Clone, Copy)]
enum Filter {
    String,
    Bool,
    Null,
    EscapeQuotes,
}
#[derive(Clone, Copy)]
enum StripMode {
    Strip,
    Log,
    Reject,
}

struct StripChars {
    ascii: [u64; 2],
    unicode: HashSet<char>,
}

impl StripChars {
    fn new(chars: &str) -> Self {
        let mut strip = Self {
            ascii: [0; 2],
            unicode: HashSet::new(),
        };
        for character in chars.chars() {
            if character.is_ascii() {
                let index = character as usize;
                strip.ascii[index / 64] |= 1 << (index % 64);
            } else {
                strip.unicode.insert(character);
            }
        }
        strip
    }

    fn contains(&self, character: char) -> bool {
        if character.is_ascii() {
            let index = character as usize;
            self.ascii[index / 64] & (1 << (index % 64)) != 0
        } else {
            self.unicode.contains(&character)
        }
    }
}

impl Custom {
    pub fn load(path: &Path) -> Result<Self, String> {
        Self::parse(
            &std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read SQL_CUSTOM configuration: {e}"))?,
        )
    }
    fn parse(text: &str) -> Result<Self, String> {
        let mut sections = BTreeMap::<String, BTreeMap<String, String>>::new();
        let mut current = None;
        for (line_no, line) in text.lines().enumerate() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                if name.is_empty() || sections.insert(name.to_owned(), BTreeMap::new()).is_some() {
                    return Err(format!(
                        "duplicate or empty section at line {}",
                        line_no + 1
                    ));
                }
                current = Some(name.to_owned());
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("invalid INI line {}", line_no + 1))?;
            let section = sections
                .get_mut(current.as_ref().ok_or("INI key outside section")?)
                .unwrap();
            if section
                .insert(key.trim().to_ascii_lowercase(), value.trim().to_owned())
                .is_some()
            {
                return Err(format!("duplicate INI key at line {}", line_no + 1));
            }
        }
        let defaults = sections
            .remove("Default")
            .ok_or("missing Default section")?;
        for (key, value) in &defaults {
            validate_setting(key, value, true)?;
        }
        if defaults.get("version").map(String::as_str) != Some("1") {
            return Err("SQL_CUSTOM Version must be 1".into());
        }
        let mut queries = HashMap::new();
        for (name, values) in sections {
            let mut settings = defaults.clone();
            let mut parts = BTreeMap::<usize, BTreeMap<usize, String>>::new();
            let mut inputs = BTreeMap::new();
            let mut output = HashMap::new();
            for (key, value) in values {
                if key == "output" {
                    for mapping in mappings(&value, false)? {
                        if output.insert(mapping.index, mapping.filters).is_some() {
                            return Err(format!("duplicate OUTPUT column in {name}"));
                        }
                    }
                } else if let Some(suffix) = key.strip_prefix("sql") {
                    let (statement, part) = suffix.split_once('_').ok_or("invalid SQL key")?;
                    let statement = positive(statement)?;
                    if part == "inputs" {
                        inputs.insert(statement, mappings(&value, true)?);
                    } else {
                        parts
                            .entry(statement)
                            .or_default()
                            .insert(positive(part)?, value);
                    }
                } else {
                    validate_setting(&key, &value, false)?;
                    settings.insert(key, value);
                }
            }
            let mut statements = Vec::new();
            for (ordinal, (number, fragments)) in parts.into_iter().enumerate() {
                if number != ordinal + 1 || fragments.keys().copied().ne(1..=fragments.len()) {
                    return Err(format!("SQL numbering gap in {name}"));
                }
                let sql = fragments.into_values().collect::<Vec<_>>().join(" ");
                if sql.contains("$CUSTOM_") {
                    return Err("SQL substitutions are unsupported; use prepared parameters".into());
                }
                statements.push(Statement {
                    sql,
                    inputs: inputs.remove(&number).unwrap_or_default(),
                });
            }
            if statements.is_empty() || !inputs.is_empty() {
                return Err(format!("missing SQL statement in {name}"));
            }
            let strip = settings
                .get("strip chars")
                .map(|s| {
                    s.strip_prefix('"')
                        .and_then(|s| s.strip_suffix('"'))
                        .unwrap_or(s)
                        .to_owned()
                })
                .unwrap_or_default();
            let strip = StripChars::new(&strip);
            let strip_mode = match settings
                .get("strip chars mode")
                .map(String::as_str)
                .unwrap_or("0")
            {
                "0" => StripMode::Strip,
                "1" => StripMode::Log,
                "2" => StripMode::Reject,
                _ => unreachable!(),
            };
            let return_id = setting_bool(&settings, "return insertid");
            let return_id_string = setting_bool(&settings, "return insertid string");
            queries.insert(
                name,
                Arc::new(Query {
                    statements,
                    output,
                    strip,
                    strip_mode,
                    insert_id: return_id || return_id_string,
                    insert_string: return_id_string && !return_id,
                }),
            );
        }
        Ok(Self { queries })
    }
}

#[cfg(feature = "synthetic-bench")]
pub fn parse_for_benchmark(text: &str) -> Result<Custom, String> {
    Custom::parse(text)
}

fn setting_bool(settings: &BTreeMap<String, String>, key: &str) -> bool {
    settings
        .get(key)
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}
fn validate_setting(key: &str, value: &str, defaults: bool) -> Result<(), String> {
    let valid = match key {
        "version" => defaults && value == "1",
        "strip chars" => true,
        "strip chars mode" => matches!(value, "0" | "1" | "2"),
        "input sqf parser" => value.eq_ignore_ascii_case("false"),
        "prepared statement" => value.eq_ignore_ascii_case("true"),
        "return insertid" | "return insertid string" => {
            value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false")
        }
        "number of retrys" => value == "0",
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!("unsupported SQL_CUSTOM setting {key}"))
    }
}
fn positive(text: &str) -> Result<usize, String> {
    text.parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| "mapping indices must be positive integers".into())
}
fn mappings(text: &str, input: bool) -> Result<Vec<Mapping>, String> {
    if text.is_empty() || (input && text.trim() == "0") {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|item| {
            let mut pieces = item.trim().split('-');
            let index = positive(pieces.next().unwrap())? - 1;
            let filters = pieces
                .map(|option| match option.trim().to_ascii_lowercase().as_str() {
                    "string" => Ok(Filter::String),
                    "bool" => Ok(Filter::Bool),
                    "null" => Ok(Filter::Null),
                    "add_escape_quotes" => Ok(Filter::EscapeQuotes),
                    _ => Err(format!("unsupported SQL_CUSTOM filter {option}")),
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(Mapping { index, filters })
        })
        .collect()
}

impl Query {
    pub fn bind(&self, data: &str) -> Result<Vec<Vec<Value>>, String> {
        let args: Vec<&str> = if data.is_empty() {
            vec![""]
        } else {
            data.split(':').collect()
        };
        self.statements.iter().map(|statement| statement.inputs.iter().map(|mapping| {
            let raw = args.get(mapping.index).ok_or("not enough SQL_CUSTOM arguments")?;
            let mut removed = 0;
            let mut value: String = raw
                .chars()
                .filter(|character| {
                    if self.strip.contains(*character) {
                        removed += 1;
                        false
                    } else {
                        true
                    }
                })
                .collect();
            if removed > 0 {
                match self.strip_mode {
                    StripMode::Strip => (),
                    StripMode::Log => eprintln!("SQL_CUSTOM stripped {removed} characters from input {}",mapping.index+1),
                    StripMode::Reject => { eprintln!("SQL_CUSTOM rejected {removed} prohibited characters in input {}",mapping.index+1); return Err("SQL_CUSTOM prohibited input characters".into()); }
                }
            }
            for filter in &mapping.filters {
                match filter {
                    Filter::Null if value.is_empty() => return Ok(Value::NULL),
                    Filter::Bool => value = if boolean(&value)? { "1" } else { "0" }.into(),
                    Filter::String => value = format!("\"{value}\""),
                    Filter::EscapeQuotes => value = value.replace('"',"\"\""),
                    Filter::Null => (),
                }
            }
            Ok(Value::Bytes(value.into_bytes()))
        }).collect()).collect()
    }
}
fn boolean(value: &str) -> Result<bool, String> {
    if value.eq_ignore_ascii_case("true") || value == "1" {
        Ok(true)
    } else if value.eq_ignore_ascii_case("false") || value == "0" {
        Ok(false)
    } else {
        Err("invalid SQL_CUSTOM boolean".into())
    }
}

fn write_value(
    out: &mut sqf::Writer,
    value: &Value,
    filters: &[Filter],
    explicit: bool,
) -> Result<(), String> {
    if matches!(value, Value::NULL) {
        return out.push(if filters.iter().any(|f| matches!(f, Filter::Null)) {
            "objNull"
        } else {
            "\"\""
        });
    }
    if let Value::Bytes(bytes) = value
        && bytes.len() > out.remaining()
    {
        return Err("response exceeds configured byte limit".into());
    }
    let text = match value {
        Value::Bytes(bytes) => {
            Cow::Borrowed(std::str::from_utf8(bytes).map_err(|_| "database value is not UTF-8")?)
        }
        _ => Cow::Owned(sqf::text(value)?),
    };
    if filters.iter().any(|f| matches!(f, Filter::Bool)) {
        return out.push(if boolean(&text)? { "true" } else { "false" });
    }
    let temporal = matches!(value, Value::Date(..) | Value::Time(..));
    if !temporal && filters.iter().any(|f| matches!(f, Filter::String)) {
        return out.quoted(&text);
    }
    if !explicit && matches!(value, Value::Bytes(_)) && !sqf::literal(&text) {
        return out.quoted(&text);
    }
    if text.is_empty() {
        return out.push("\"\"");
    }
    if !sqf::literal(&text) {
        return Err("database value is not a safe SQF literal; use OUTPUT STRING".into());
    }
    out.push(&text)
}

fn append_row<'a>(
    rows: &mut sqf::Writer,
    first: &mut bool,
    values: impl Iterator<Item = &'a Value>,
    query: &Query,
) -> Result<(), String> {
    if !*first {
        rows.push(",")?;
    }
    *first = false;
    rows.push("[")?;
    for (index, value) in values.enumerate() {
        if index > 0 {
            rows.push(",")?;
        }
        let filters = query.output.get(&index);
        write_value(
            rows,
            value,
            filters.map(Vec::as_slice).unwrap_or(&[]),
            filters.is_some(),
        )?;
    }
    rows.push("]")
}

fn finish_response(
    query: &Query,
    mut rows: sqf::Writer,
    insert_id: u64,
    max_bytes: usize,
) -> Result<String, String> {
    let mut prefix = sqf::Writer::new(max_bytes);
    prefix.push("[1,[")?;
    if query.insert_id {
        if query.insert_string {
            prefix.quoted(&insert_id.to_string())?;
        } else {
            prefix.push(&insert_id.to_string())?;
        }
        prefix.push(",[")?;
    }
    rows.prepend(&prefix.finish())?;
    rows.push(if query.insert_id { "]]]" } else { "]]" })?;
    Ok(rows.finish())
}

#[cfg(any(feature = "synthetic-bench", test))]
pub fn format_rows_for_benchmark(
    query: &Query,
    values: &[Vec<Value>],
    insert_id: u64,
    max_bytes: usize,
) -> Result<String, String> {
    let mut rows = sqf::Writer::new(max_bytes);
    let mut first = true;
    for row in values {
        append_row(&mut rows, &mut first, row.iter(), query)?;
    }
    finish_response(query, rows, insert_id, max_bytes)
}

#[cfg(not(feature = "profile-requests"))]
pub async fn execute(
    pool: &Pool,
    query: &Query,
    inputs: Vec<Vec<Value>>,
    max_bytes: usize,
) -> Result<String, String> {
    execute_inner(pool, query, inputs, max_bytes).await
}

#[cfg(feature = "profile-requests")]
pub async fn execute_profiled(
    pool: &Pool,
    query: &Query,
    inputs: Vec<Vec<Value>>,
    max_bytes: usize,
) -> (Result<String, String>, ProfileExecution) {
    let mut profile = ProfileExecution::default();
    let result = execute_inner(pool, query, inputs, max_bytes, &mut profile).await;
    (result, profile)
}

async fn execute_inner(
    pool: &Pool,
    query: &Query,
    inputs: Vec<Vec<Value>>,
    max_bytes: usize,
    #[cfg(feature = "profile-requests")] profile: &mut ProfileExecution,
) -> Result<String, String> {
    if inputs.len() != query.statements.len() {
        return Err("invalid bound statement count".into());
    }
    #[cfg(feature = "profile-requests")]
    let pool_started = Instant::now();
    let connection = pool.get_conn().await;
    #[cfg(feature = "profile-requests")]
    add_micros(&mut profile.pool_acquire_us, pool_started.elapsed());
    let mut conn =
        connection.map_err(|_| "Error MariaDBConnectionException0 Exception".to_string())?;
    let mut rows = sqf::Writer::new(max_bytes);
    let mut first = true;
    let mut insert_id = 0;
    for (statement, params) in query.statements.iter().zip(inputs) {
        #[cfg(feature = "profile-requests")]
        let query_started = Instant::now();
        let query_result = conn
            .exec_iter(&statement.sql, Params::Positional(params))
            .await;
        #[cfg(feature = "profile-requests")]
        add_micros(&mut profile.mysql_async_us, query_started.elapsed());
        let mut result =
            query_result.map_err(|_| "Error MariaDBStatementException0 Exception".to_string())?;
        if let Some(id) = result.last_insert_id() {
            insert_id = id;
        }
        loop {
            #[cfg(feature = "profile-requests")]
            let next_started = Instant::now();
            let next_row = result.next().await;
            #[cfg(feature = "profile-requests")]
            add_micros(&mut profile.mysql_async_us, next_started.elapsed());
            let Some(row) =
                next_row.map_err(|_| "Error MariaDBStatementException0 Exception".to_string())?
            else {
                break;
            };
            #[cfg(feature = "profile-requests")]
            let format_started = Instant::now();
            append_row(&mut rows, &mut first, row.unwrap().iter(), query)
                .map_err(|_| "Error extDB3Exception Exception")?;
            #[cfg(feature = "profile-requests")]
            {
                add_micros(&mut profile.format_us, format_started.elapsed());
                profile.rows += 1;
            }
        }
        #[cfg(feature = "profile-requests")]
        let drop_started = Instant::now();
        let drop_result = result.drop_result().await;
        #[cfg(feature = "profile-requests")]
        add_micros(&mut profile.mysql_async_us, drop_started.elapsed());
        drop_result.map_err(|_| "Error MariaDBStatementException0 Exception".to_string())?;
    }
    #[cfg(feature = "profile-requests")]
    let format_started = Instant::now();
    let response = finish_response(query, rows, insert_id, max_bytes)
        .map_err(|_| "Error extDB3Exception Exception".to_string());
    #[cfg(feature = "profile-requests")]
    add_micros(&mut profile.format_us, format_started.elapsed());
    response
}

#[cfg(feature = "profile-requests")]
fn add_micros(total: &mut u64, duration: Duration) {
    *total = total.saturating_add(duration.as_micros().min(u64::MAX as u128) as u64);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parameter_reordering_null_and_bool() {
        let custom = Custom::parse("[Default]\nVersion=1\nStrip Chars=\";\"\n[q]\nSQL1_1=SELECT ?,?,?,?\nSQL1_INPUTS=2,1-BOOL,3-NULL,2\n").unwrap();
        assert_eq!(
            custom.queries["q"].bind("true:ab;c:").unwrap(),
            vec![vec![
                Value::from("abc"),
                Value::from("1"),
                Value::NULL,
                Value::from("abc")
            ]]
        );
        assert!(custom.queries["q"].bind("maybe:x:").is_err());
        assert!(custom.queries["q"].bind("true").is_err());
    }
    #[test]
    fn strip_character_set_matches_unicode_and_ascii() {
        let custom = Custom::parse(
            "[Default]\nVersion=1\nStrip Chars=;é\n[q]\nSQL1_1=SELECT ?\nSQL1_INPUTS=1\n",
        )
        .unwrap();
        assert_eq!(
            custom.queries["q"].bind("a;bé").unwrap(),
            vec![vec![Value::from("ab")]]
        );
    }
    #[test]
    fn synthetic_row_formatter_preserves_output_filters_and_insert_id() {
        let custom = Custom::parse(
            "[Default]\nVersion=1\nReturn InsertID=true\n[q]\nSQL1_1=SELECT ?,?,?\nOUTPUT=1-STRING,2-BOOL,3-NULL\n",
        )
        .unwrap();
        let rows = vec![vec![
            Value::from("foo\"bar"),
            Value::from("true"),
            Value::NULL,
        ]];
        assert_eq!(
            format_rows_for_benchmark(&custom.queries["q"], &rows, 7, 1024).unwrap(),
            "[1,[7,[[\"foo\"\"bar\",true,objNull]]]]"
        );
    }
    #[test]
    fn byte_result_formatting_rejects_invalid_utf8() {
        let mut out = sqf::Writer::new(32);
        assert_eq!(
            write_value(&mut out, &Value::Bytes(vec![0xff]), &[], false).unwrap_err(),
            "database value is not UTF-8"
        );
    }
    #[test]
    fn reject_unknown_options_and_replay() {
        for option in [
            "Prepared Statement=false",
            "Number of Retrys=5",
            "Input SQF Parser=true",
            "Typo=true",
        ] {
            assert!(
                Custom::parse(&format!(
                    "[Default]\nVersion=1\n[q]\nSQL1_1=SELECT 1\n{option}"
                ))
                .is_err()
            );
        }
    }
    #[test]
    fn raw_arrays_stay_arrays_and_strings_cannot_escape() {
        let mut out = sqf::Writer::new(100);
        write_value(&mut out, &Value::from("['rifle',2]"), &[], true).unwrap();
        assert_eq!(out.finish(), "['rifle',2]");
        let mut out = sqf::Writer::new(100);
        assert!(write_value(&mut out, &Value::from("[];call evil"), &[], true).is_err());
        let mut out = sqf::Writer::new(100);
        write_value(
            &mut out,
            &Value::from("x\";call evil"),
            &[Filter::String, Filter::EscapeQuotes],
            true,
        )
        .unwrap();
        assert_eq!(out.finish(), "\"x\"\";call evil\"");
    }
    #[test]
    fn null_output_contract() {
        let mut out = sqf::Writer::new(30);
        write_value(&mut out, &Value::NULL, &[], true).unwrap();
        assert_eq!(out.finish(), "\"\"");
        let mut out = sqf::Writer::new(30);
        write_value(&mut out, &Value::NULL, &[Filter::Null], true).unwrap();
        assert_eq!(out.finish(), "objNull");
    }
    #[test]
    fn datetime_stays_an_sqf_date_array_with_string_output_filter() {
        let mut out = sqf::Writer::new(100);
        write_value(
            &mut out,
            &Value::Date(2026, 10, 9, 12, 0, 0, 0),
            &[Filter::String],
            true,
        )
        .unwrap();
        assert_eq!(out.finish(), "[2026,10,9,12,0,0]");
    }
    #[test]
    fn consumer_configuration_if_supplied() {
        if let Ok(path) = std::env::var("SQL_CUSTOM_TEST_CONFIG") {
            let custom = Custom::load(Path::new(&path)).unwrap();
            assert!(custom.queries.contains_key("loadPlayer"));
            assert_eq!(
                custom.queries["insertOrUpdateObjVarBatch8"]
                    .statements
                    .len(),
                8
            );
            assert_eq!(
                custom.queries["resetACEMedical"].bind("").unwrap(),
                vec![Vec::<Value>::new()]
            );
        }
    }
}
