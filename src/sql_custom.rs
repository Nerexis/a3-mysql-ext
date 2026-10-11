use crate::compat::{Failure, error_code};
#[cfg(feature = "profile-requests")]
use crate::profile::Execution as ProfileExecution;
use crate::sqf;
use md5::{Digest, Md5};
use mysql_async::{
    BinaryProtocol, Params, Pool, QueryResult, TextProtocol, Value, consts::ColumnType,
    prelude::Queryable,
};
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
    input_sqf_parser: bool,
    prepared_statement: bool,
    retries: usize,
    insert_id: bool,
    insert_string: bool,
}
struct Statement {
    sql: String,
    inputs: Vec<Mapping>,
}
enum CustomResult<'a> {
    Binary(QueryResult<'a, 'static, BinaryProtocol>),
    Text(QueryResult<'a, 'static, TextProtocol>),
}

impl CustomResult<'_> {
    fn last_insert_id(&self) -> Option<u64> {
        match self {
            Self::Binary(result) => result.last_insert_id(),
            Self::Text(result) => result.last_insert_id(),
        }
    }

    async fn next(&mut self) -> mysql_async::Result<Option<mysql_async::Row>> {
        match self {
            Self::Binary(result) => result.next().await,
            Self::Text(result) => result.next().await,
        }
    }

    async fn drop_result(self) -> mysql_async::Result<()> {
        match self {
            Self::Binary(result) => result.drop_result().await,
            Self::Text(result) => result.drop_result().await,
        }
    }
}

#[derive(Debug)]
pub struct BoundStatement {
    sql: String,
    params: Vec<Value>,
}

impl BoundStatement {
    pub fn params(&self) -> &[Value] {
        &self.params
    }
}

impl PartialEq<Vec<Value>> for BoundStatement {
    fn eq(&self, other: &Vec<Value>) -> bool {
        &self.params == other
    }
}
struct Mapping {
    index: usize,
    filters: Vec<Filter>,
}
#[derive(Clone, Copy)]
enum Filter {
    String,
    String2,
    Bool,
    Beguid,
    Null,
    Time,
    Strip,
    MysqlEscape,
    RemoveQuotes,
    EscapeQuotes,
    RemoveEscapeQuotes,
    StringEscapeQuotes,
    StringEscapeQuotes2,
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

#[derive(Clone, Copy)]
pub(crate) struct RawOptions {
    quote: Option<char>,
    null_as_objnull: bool,
}

impl RawOptions {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        let (quote, null_as_objnull) = match value.to_ascii_uppercase().as_str() {
            "" => (None, false),
            "TEXT" => (Some('"'), false),
            "TEXT2" => (Some('\''), false),
            "NULL" => (None, true),
            "TEXT-NULL" => (Some('"'), true),
            _ => return Err("unsupported SQL protocol option".into()),
        };
        Ok(Self {
            quote,
            null_as_objnull,
        })
    }
}

pub(crate) enum ProtocolDefinition {
    Raw(RawOptions),
    Custom(Arc<Custom>),
}

pub(crate) enum Operation {
    Raw {
        sql: String,
        options: RawOptions,
    },
    Custom {
        query: Arc<Query>,
        statements: Vec<BoundStatement>,
    },
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
            let input_sqf_parser = setting_bool_default(&settings, "input sqf parser", false);
            let prepared_statement = setting_bool_default(&settings, "prepared statement", true);
            let retries = settings
                .get("number of retrys")
                .map(|value| value.parse::<usize>())
                .transpose()
                .map_err(|_| "invalid Number of Retrys")?
                .unwrap_or(5);
            if prepared_statement
                && statements
                    .iter()
                    .any(|statement| statement.sql.contains("$CUSTOM_"))
            {
                return Err("SQL substitutions require Prepared Statement=false".into());
            }
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
                    input_sqf_parser,
                    prepared_statement,
                    retries,
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
fn setting_bool_default(settings: &BTreeMap<String, String>, key: &str, default: bool) -> bool {
    settings
        .get(key)
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(default)
}
fn validate_setting(key: &str, value: &str, defaults: bool) -> Result<(), String> {
    let valid = match key {
        "version" => defaults && value == "1",
        "strip chars" => true,
        "strip chars mode" => matches!(value, "0" | "1" | "2"),
        "input sqf parser" | "prepared statement" => {
            value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false")
        }
        "return insertid" | "return insertid string" => {
            value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false")
        }
        "number of retrys" => value.parse::<usize>().is_ok(),
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
                    "string2" => Ok(Filter::String2),
                    "bool" => Ok(Filter::Bool),
                    "null" => Ok(Filter::Null),
                    "time" => Ok(Filter::Time),
                    "strip" => Ok(Filter::Strip),
                    "mysql_escape" => Ok(Filter::MysqlEscape),
                    "beguid" => Ok(Filter::Beguid),
                    "remove_quotes" => Ok(Filter::RemoveQuotes),
                    "add_escape_quotes" => Ok(Filter::EscapeQuotes),
                    "remove_escape_quotes" => Ok(Filter::RemoveEscapeQuotes),
                    "string_escape_quotes" => Ok(Filter::StringEscapeQuotes),
                    "string_escape_quotes2" => Ok(Filter::StringEscapeQuotes2),
                    _ => Err(format!("unsupported SQL_CUSTOM filter {option}")),
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(Mapping { index, filters })
        })
        .collect()
}

impl Query {
    pub fn bind(&self, data: &str) -> Result<Vec<BoundStatement>, Failure> {
        let args: Vec<String> = if self.input_sqf_parser {
            sqf_array_values(data)
                .map_err(|detail| Failure::new(error_code::INVALID_FORMAT, detail))?
        } else if data.is_empty() {
            vec![String::new()]
        } else {
            data.split(':').map(str::to_owned).collect()
        };
        self.statements
            .iter()
            .map(|statement| {
                let params = statement
                    .inputs
                    .iter()
                    .map(|mapping| {
                        let raw = args.get(mapping.index).ok_or_else(|| {
                            Failure::new(
                                error_code::INVALID_FORMAT,
                                "not enough SQL_CUSTOM arguments",
                            )
                        })?;
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
                                StripMode::Log => eprintln!(
                                    "SQL_CUSTOM stripped {removed} characters from input {}",
                                    mapping.index + 1
                                ),
                                StripMode::Reject => {
                                    return Err(Failure::new(
                                        error_code::STRIP_CHAR_FOUND,
                                        format!(
                                            "rejected {removed} prohibited characters in input {}",
                                            mapping.index + 1
                                        ),
                                    ));
                                }
                            }
                        }
                        for filter in &mapping.filters {
                            match filter {
                                Filter::Null if value.is_empty() => return Ok(Value::NULL),
                                Filter::Bool => {
                                    value = if boolean(&value).map_err(|detail| {
                                        Failure::new(error_code::INVALID_FORMAT, detail)
                                    })? {
                                        "1"
                                    } else {
                                        "0"
                                    }
                                    .into()
                                }
                                Filter::String => value = format!("\"{value}\""),
                                Filter::String2 => value = format!("'{value}'"),
                                Filter::Time if self.prepared_statement => {
                                    return parse_time_value(&value).map_err(|detail| {
                                        Failure::new(error_code::INVALID_FORMAT, detail)
                                    });
                                }
                                Filter::Time => {
                                    return Err(Failure::new(
                                        error_code::INVALID_FORMAT,
                                        "time is only supported with Prepared Statement=true",
                                    ));
                                }
                                Filter::MysqlEscape if !self.prepared_statement => {
                                    value = mysql_escape(&value);
                                }
                                Filter::MysqlEscape => {
                                    return Err(Failure::new(
                                        error_code::INVALID_FORMAT,
                                        "mysql_escape is only supported with Prepared Statement=false",
                                    ));
                                }
                                Filter::Beguid => {
                                    value = beguid(&value).map_err(|detail| {
                                        Failure::new(error_code::INVALID_FORMAT, detail)
                                    })?;
                                }
                                Filter::RemoveQuotes => {
                                    value.retain(|character| !matches!(character, '\'' | '"'));
                                }
                                Filter::EscapeQuotes => value = value.replace('"', "\"\""),
                                Filter::RemoveEscapeQuotes => value = value.replace("\"\"", "\""),
                                Filter::StringEscapeQuotes => {
                                    value = string_escape_quotes(&value, '"');
                                }
                                Filter::StringEscapeQuotes2 => {
                                    value = string_escape_quotes(&value, '\'');
                                }
                                Filter::Strip => (),
                                Filter::Null => (),
                            }
                        }
                        Ok(Value::Bytes(value.into_bytes()))
                    })
                    .collect::<Result<Vec<_>, Failure>>()?;
                let sql = if self.prepared_statement {
                    statement.sql.clone()
                } else {
                    substitute_custom(&statement.sql, &statement.inputs, &params)?
                };
                Ok(BoundStatement { sql, params })
            })
            .collect()
    }
}

fn sqf_array_values(input: &str) -> Result<Vec<String>, String> {
    if !sqf::literal(input) {
        return Err("Input SQF Parser expected an SQF literal array".into());
    }
    let input = input.trim();
    if !input.starts_with('[') || !input.ends_with(']') {
        return Err("Input SQF Parser expected an array".into());
    }
    let bytes = input.as_bytes();
    let mut values = Vec::new();
    let mut start = 1;
    let mut depth = 0usize;
    let mut quote = None;
    let mut index = 1usize;
    while index + 1 < bytes.len() {
        let byte = bytes[index];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                if bytes.get(index + 1) == Some(&delimiter) {
                    index += 1;
                } else {
                    quote = None;
                }
            }
        } else {
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'[' => depth += 1,
                b']' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    values.push(decode_sqf_value(&input[start..index])?);
                    start = index + 1;
                }
                _ => (),
            }
        }
        index += 1;
    }
    if start < bytes.len() - 1 {
        values.push(decode_sqf_value(&input[start..bytes.len() - 1])?);
    }
    Ok(values)
}

fn decode_sqf_value(input: &str) -> Result<String, String> {
    let value = input.trim();
    let Some(delimiter) = value.as_bytes().first().copied() else {
        return Err("Input SQF Parser found an empty array element".into());
    };
    if (delimiter != b'\'' && delimiter != b'"') || value.as_bytes().last() != Some(&delimiter) {
        return Ok(value.to_owned());
    }
    let contents = &value[1..value.len() - 1];
    let mut decoded = String::with_capacity(contents.len());
    let mut characters = contents.chars().peekable();
    while let Some(character) = characters.next() {
        if character as u32 == delimiter as u32 && characters.peek() == Some(&character) {
            characters.next();
        }
        decoded.push(character);
    }
    Ok(decoded)
}

fn parse_time_value(value: &str) -> Result<Value, String> {
    let contents = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or("TIME input must be [year,month,day,hour,minute,second]")?;
    let fields = contents
        .split(',')
        .map(|field| field.trim().parse::<i64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "TIME input contains a non-integer")?;
    let [year, month, day, hour, minute, second]: [i64; 6] = fields
        .try_into()
        .map_err(|fields: Vec<i64>| format!("TIME input needs 6 integers, got {}", fields.len()))?;
    if year != 0 || month != 0 {
        return Err("TIME year and month fields are unsupported".into());
    }
    let day = u8::try_from(day).map_err(|_| "TIME day is out of range")?;
    let hour = u8::try_from(hour).map_err(|_| "TIME hour is out of range")?;
    let minute = u8::try_from(minute).map_err(|_| "TIME minute is out of range")?;
    let second = u8::try_from(second).map_err(|_| "TIME second is out of range")?;
    Ok(Value::Time(false, u32::from(day), hour, minute, second, 0))
}

fn beguid(value: &str) -> Result<String, String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("BEGUID input must be a SteamID64 decimal string".into());
    }
    let steam_id = value
        .parse::<u64>()
        .map_err(|_| "BEGUID SteamID64 is out of range")?;
    let mut input = [0; 10];
    input[..2].copy_from_slice(b"BE");
    input[2..].copy_from_slice(&steam_id.to_le_bytes());
    Ok(format!("{:x}", Md5::digest(input)))
}

fn mysql_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\0' => escaped.push_str("\\0"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\\' => escaped.push_str("\\\\"),
            '\'' => escaped.push_str("\\'"),
            '"' => escaped.push_str("\\\""),
            '\u{1a}' => escaped.push_str("\\Z"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn string_escape_quotes(value: &str, delimiter: char) -> String {
    let escaped = value.replace('\'', "''").replace('"', "\"\"");
    format!("{delimiter}{escaped}{delimiter}")
}

fn sql_value(value: &Value) -> Result<String, Failure> {
    match value {
        Value::NULL => Ok("NULL".into()),
        Value::Bytes(bytes) => std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| Failure::new(error_code::INVALID_FORMAT, "SQL input is not UTF-8")),
        _ => sqf::text(value).map_err(|detail| Failure::new(error_code::INVALID_FORMAT, detail)),
    }
}

fn substitute_custom(sql: &str, inputs: &[Mapping], params: &[Value]) -> Result<String, Failure> {
    let replacements = inputs
        .iter()
        .zip(params)
        .map(|(mapping, value)| Ok((mapping.index + 1, sql_value(value)?)))
        .collect::<Result<HashMap<_, _>, Failure>>()?;
    let mut output = String::with_capacity(sql.len());
    let mut index = 0;
    while index < sql.len() {
        let remainder = &sql[index..];
        if let Some(digits) = remainder.strip_prefix("$CUSTOM_") {
            let end = digits.find('$').ok_or_else(|| {
                Failure::new(error_code::INVALID_FORMAT, "unterminated SQL substitution")
            })?;
            let number = digits[..end]
                .parse::<usize>()
                .ok()
                .filter(|number| *number > 0)
                .ok_or_else(|| {
                    Failure::new(error_code::INVALID_FORMAT, "invalid SQL substitution index")
                })?;
            let value = replacements.get(&number).ok_or_else(|| {
                Failure::new(
                    error_code::INVALID_FORMAT,
                    format!("SQL substitution CUSTOM_{number} has no input mapping"),
                )
            })?;
            output.push_str(value);
            index += "$CUSTOM_".len() + end + 1;
            continue;
        }
        let character = remainder.chars().next().expect("non-empty SQL remainder");
        output.push(character);
        index += character.len_utf8();
    }
    Ok(output)
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

#[cfg(test)]
fn write_value(
    out: &mut sqf::Writer,
    value: &Value,
    filters: &[Filter],
    explicit: bool,
) -> Result<(), String> {
    write_value_with(out, value, filters, explicit, false, None, None)
}

fn write_value_with(
    out: &mut sqf::Writer,
    value: &Value,
    filters: &[Filter],
    explicit: bool,
    null_as_objnull: bool,
    forced_quote: Option<char>,
    strip: Option<&StripChars>,
) -> Result<(), String> {
    if matches!(value, Value::NULL) {
        return out.push(
            if null_as_objnull || filters.iter().any(|f| matches!(f, Filter::Null)) {
                "objNull"
            } else {
                "\"\""
            },
        );
    }
    if let Value::Bytes(bytes) = value
        && bytes.len() > out.remaining()
    {
        return Err("response exceeds configured byte limit".into());
    }
    let mut text = match value {
        Value::Bytes(bytes) => {
            Cow::Borrowed(std::str::from_utf8(bytes).map_err(|_| "database value is not UTF-8")?)
        }
        _ => Cow::Owned(sqf::text(value)?),
    };
    let mut quote = forced_quote;
    let mut prequoted = false;
    let mut preescaped = false;
    for filter in filters {
        match filter {
            Filter::String => quote = Some('"'),
            Filter::String2 => quote = Some('\''),
            Filter::EscapeQuotes => {
                text = Cow::Owned(text.replace('"', "\"\""));
                preescaped = true;
            }
            Filter::RemoveEscapeQuotes => {
                let replacement = text.replace("\"\"", "\"");
                text = Cow::Owned(replacement);
            }
            Filter::RemoveQuotes => {
                text.to_mut()
                    .retain(|character| !matches!(character, '\'' | '"'));
            }
            Filter::StringEscapeQuotes => {
                text = Cow::Owned(string_escape_quotes(&text, '"'));
                prequoted = true;
            }
            Filter::StringEscapeQuotes2 => {
                text = Cow::Owned(string_escape_quotes(&text, '\''));
                prequoted = true;
            }
            Filter::Beguid => text = Cow::Owned(beguid(&text)?),
            Filter::Strip => {
                if let Some(strip) = strip {
                    text = Cow::Owned(
                        text.chars()
                            .filter(|character| !strip.contains(*character))
                            .collect(),
                    );
                }
            }
            Filter::Bool | Filter::Null | Filter::Time | Filter::MysqlEscape => (),
        }
    }
    if filters.iter().any(|filter| matches!(filter, Filter::Bool)) {
        return out.push(if boolean(&text)? { "true" } else { "false" });
    }
    let temporal = matches!(value, Value::Date(..) | Value::Time(..));
    if prequoted {
        if !sqf::literal(&text) {
            return Err("output filter produced an invalid SQF string".into());
        }
        return out.push(&text);
    }
    if let Some(delimiter) = quote.filter(|_| !temporal) {
        if preescaped && delimiter == '"' {
            out.push("\"")?;
            out.push(&text)?;
            return out.push("\"");
        }
        return out.quoted_with(&text, delimiter);
    }
    let string_transform = filters.iter().any(|filter| {
        matches!(
            filter,
            Filter::RemoveQuotes | Filter::EscapeQuotes | Filter::RemoveEscapeQuotes
        )
    });
    if (!explicit || string_transform) && matches!(value, Value::Bytes(_)) && !sqf::literal(&text) {
        if preescaped {
            out.push("\"")?;
            out.push(&text)?;
            return out.push("\"");
        }
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
        write_value_with(
            rows,
            value,
            filters.map(Vec::as_slice).unwrap_or(&[]),
            filters.is_some(),
            false,
            None,
            Some(&query.strip),
        )?;
    }
    rows.push("]")
}

fn append_raw_row<'a>(
    rows: &mut sqf::Writer,
    first: &mut bool,
    columns: &[mysql_async::Column],
    values: impl Iterator<Item = &'a Value>,
    options: RawOptions,
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
        let column_type = columns
            .get(index)
            .map(|column| column.column_type())
            .unwrap_or(ColumnType::MYSQL_TYPE_UNKNOWN);
        write_raw_value(rows, value, column_type, options)?;
    }
    rows.push("]")
}

fn write_raw_value(
    out: &mut sqf::Writer,
    value: &Value,
    column_type: ColumnType,
    options: RawOptions,
) -> Result<(), String> {
    if matches!(value, Value::NULL) {
        return out.push(if options.null_as_objnull {
            "objNull"
        } else {
            "\"\""
        });
    }
    if let Some(delimiter) = options.quote
        && raw_text_type(column_type)
    {
        let Value::Bytes(bytes) = value else {
            return write_value_with(out, value, &[], false, options.null_as_objnull, None, None);
        };
        let text = std::str::from_utf8(bytes).map_err(|_| "database value is not UTF-8")?;
        return out.quoted_with(text, delimiter);
    }
    write_value_with(out, value, &[], false, options.null_as_objnull, None, None)
}

fn raw_text_type(column_type: ColumnType) -> bool {
    matches!(
        column_type,
        ColumnType::MYSQL_TYPE_VARCHAR
            | ColumnType::MYSQL_TYPE_VAR_STRING
            | ColumnType::MYSQL_TYPE_STRING
            | ColumnType::MYSQL_TYPE_ENUM
            | ColumnType::MYSQL_TYPE_SET
            | ColumnType::MYSQL_TYPE_JSON
    )
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
    operation: Operation,
    max_bytes: usize,
) -> Result<String, Failure> {
    execute_inner(pool, &operation, max_bytes).await
}

#[cfg(feature = "profile-requests")]
pub async fn execute_profiled(
    pool: &Pool,
    operation: Operation,
    max_bytes: usize,
) -> (Result<String, Failure>, ProfileExecution) {
    let mut profile = ProfileExecution::default();
    let result = execute_inner(pool, &operation, max_bytes, &mut profile).await;
    (result, profile)
}

pub(crate) fn bind(
    definition: &ProtocolDefinition,
    payload: &str,
) -> Result<(String, Operation), Failure> {
    match definition {
        ProtocolDefinition::Raw(options) => Ok((
            "raw SQL".into(),
            Operation::Raw {
                sql: payload.to_owned(),
                options: *options,
            },
        )),
        ProtocolDefinition::Custom(custom) => {
            let (query_name, data) = payload.split_once(':').unwrap_or((payload, ""));
            let query = custom.queries.get(query_name).ok_or_else(|| {
                Failure::new(
                    error_code::NO_CUSTOM_CALL,
                    format!("No Custom Call Not Found: Callname {query_name}"),
                )
            })?;
            let statements = query.bind(data)?;
            Ok((
                query_name.to_owned(),
                Operation::Custom {
                    query: Arc::clone(query),
                    statements,
                },
            ))
        }
    }
}

async fn execute_inner(
    pool: &Pool,
    operation: &Operation,
    max_bytes: usize,
    #[cfg(feature = "profile-requests")] profile: &mut ProfileExecution,
) -> Result<String, Failure> {
    let retries = match operation {
        Operation::Raw { .. } => 0,
        Operation::Custom { query, .. } => query.retries,
    };
    let mut attempt = 0;
    loop {
        let result = execute_attempt(
            pool,
            operation,
            max_bytes,
            #[cfg(feature = "profile-requests")]
            profile,
        )
        .await;
        match result {
            Ok(value) => return Ok(value),
            Err(failure) if attempt < retries && retryable(&failure) => {
                attempt = attempt.saturating_add(1);
            }
            Err(failure) => return Err(failure),
        }
    }
}

fn retryable(failure: &Failure) -> bool {
    matches!(
        failure.code,
        error_code::MARIADB_CONNECTOR
            | error_code::MARIADB_STATEMENT_0
            | error_code::MARIADB_STATEMENT_1
    )
}

async fn execute_attempt(
    pool: &Pool,
    operation: &Operation,
    max_bytes: usize,
    #[cfg(feature = "profile-requests")] profile: &mut ProfileExecution,
) -> Result<String, Failure> {
    #[cfg(feature = "profile-requests")]
    let pool_started = Instant::now();
    let connection = pool.get_conn().await;
    #[cfg(feature = "profile-requests")]
    add_micros(&mut profile.pool_acquire_us, pool_started.elapsed());
    let mut conn = connection
        .map_err(|error| Failure::new(error_code::MARIADB_CONNECTOR, error.to_string()))?;
    let mut rows = sqf::Writer::new(max_bytes);
    let mut first = true;
    let mut insert_id = 0;
    match operation {
        Operation::Raw { sql, options } => {
            #[cfg(feature = "profile-requests")]
            let query_started = Instant::now();
            let query_result = conn.query_iter(sql).await;
            #[cfg(feature = "profile-requests")]
            add_micros(&mut profile.mysql_async_us, query_started.elapsed());
            let mut result = query_result.map_err(|error| {
                Failure::new(error_code::MARIADB_STATEMENT_1, error.to_string())
            })?;
            while !result.is_empty() {
                loop {
                    #[cfg(feature = "profile-requests")]
                    let next_started = Instant::now();
                    let next_row = result.next().await;
                    #[cfg(feature = "profile-requests")]
                    add_micros(&mut profile.mysql_async_us, next_started.elapsed());
                    let Some(row) = next_row.map_err(|error| {
                        Failure::new(error_code::MARIADB_STATEMENT_1, error.to_string())
                    })?
                    else {
                        break;
                    };
                    let columns = row.columns();
                    let values = row.unwrap();
                    #[cfg(feature = "profile-requests")]
                    let format_started = Instant::now();
                    append_raw_row(&mut rows, &mut first, &columns, values.iter(), *options)
                        .map_err(|detail| Failure::new(error_code::EXTDB3, detail))?;
                    #[cfg(feature = "profile-requests")]
                    {
                        add_micros(&mut profile.format_us, format_started.elapsed());
                        profile.rows += 1;
                    }
                }
            }
            #[cfg(feature = "profile-requests")]
            let drop_started = Instant::now();
            result.drop_result().await.map_err(|error| {
                Failure::new(error_code::MARIADB_STATEMENT_1, error.to_string())
            })?;
            #[cfg(feature = "profile-requests")]
            add_micros(&mut profile.mysql_async_us, drop_started.elapsed());
        }
        Operation::Custom { query, statements } => {
            if statements.len() != query.statements.len() {
                return Err(Failure::new(
                    error_code::INVALID_FORMAT,
                    "invalid bound statement count",
                ));
            }
            for (statement, bound) in query.statements.iter().zip(statements) {
                #[cfg(feature = "profile-requests")]
                let query_started = Instant::now();
                let result = if query.prepared_statement {
                    let prepared_statement = conn.prep(&bound.sql).await.map_err(|error| {
                        Failure::new(error_code::MARIADB_STATEMENT_0, error.to_string())
                    })?;
                    let result = conn
                        .exec_iter(prepared_statement, Params::Positional(bound.params.clone()))
                        .await
                        .map_err(|error| {
                            Failure::new(error_code::MARIADB_STATEMENT_1, error.to_string())
                        })?;
                    CustomResult::Binary(result)
                } else {
                    let result = conn.query_iter(&bound.sql).await.map_err(|error| {
                        Failure::new(error_code::MARIADB_STATEMENT_1, error.to_string())
                    })?;
                    CustomResult::Text(result)
                };
                #[cfg(feature = "profile-requests")]
                add_micros(&mut profile.mysql_async_us, query_started.elapsed());
                consume_custom_result(
                    result,
                    &mut rows,
                    &mut first,
                    query,
                    &mut insert_id,
                    #[cfg(feature = "profile-requests")]
                    profile,
                )
                .await?;
                let _ = statement;
            }
        }
    }
    #[cfg(feature = "profile-requests")]
    let format_started = Instant::now();
    let response = match operation {
        Operation::Raw { .. } => rows
            .prepend("[1,[")
            .and_then(|_| rows.push("]]"))
            .map(|_| rows.finish()),
        Operation::Custom { query, .. } => finish_response(query, rows, insert_id, max_bytes),
    }
    .map_err(|detail| Failure::new(error_code::EXTDB3, detail));
    #[cfg(feature = "profile-requests")]
    add_micros(&mut profile.format_us, format_started.elapsed());
    response
}

async fn consume_custom_result(
    mut result: CustomResult<'_>,
    rows: &mut sqf::Writer,
    first: &mut bool,
    query: &Query,
    insert_id: &mut u64,
    #[cfg(feature = "profile-requests")] profile: &mut ProfileExecution,
) -> Result<(), Failure> {
    if let Some(id) = result.last_insert_id() {
        *insert_id = id;
    }
    loop {
        #[cfg(feature = "profile-requests")]
        let next_started = Instant::now();
        let next_row = result.next().await;
        #[cfg(feature = "profile-requests")]
        add_micros(&mut profile.mysql_async_us, next_started.elapsed());
        let Some(row) = next_row
            .map_err(|error| Failure::new(error_code::MARIADB_STATEMENT_1, error.to_string()))?
        else {
            break;
        };
        let values = row.unwrap();
        #[cfg(feature = "profile-requests")]
        let format_started = Instant::now();
        append_row(rows, first, values.iter(), query)
            .map_err(|detail| Failure::new(error_code::EXTDB3, detail))?;
        #[cfg(feature = "profile-requests")]
        {
            add_micros(&mut profile.format_us, format_started.elapsed());
            profile.rows += 1;
        }
    }
    #[cfg(feature = "profile-requests")]
    let drop_started = Instant::now();
    result
        .drop_result()
        .await
        .map_err(|error| Failure::new(error_code::MARIADB_STATEMENT_1, error.to_string()))?;
    #[cfg(feature = "profile-requests")]
    add_micros(&mut profile.mysql_async_us, drop_started.elapsed());
    Ok(())
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
    fn bind_errors_keep_extdb3_codes_and_retain_diagnostic_detail() {
        let strip = Custom::parse(
            "[Default]\nVersion=1\nStrip Chars=;\nStrip Chars Mode=2\n[q]\nSQL1_1=SELECT ?\nSQL1_INPUTS=1\n",
        )
        .unwrap();
        let strip_failure = strip.queries["q"].bind("a;b").unwrap_err();
        assert_eq!(strip_failure.code, error_code::STRIP_CHAR_FOUND);
        assert!(strip_failure.detail.contains("1 prohibited characters"));

        let boolean =
            Custom::parse("[Default]\nVersion=1\n[q]\nSQL1_1=SELECT ?\nSQL1_INPUTS=1-BOOL\n")
                .unwrap();
        let format_failure = boolean.queries["q"].bind("maybe").unwrap_err();
        assert_eq!(format_failure.code, error_code::INVALID_FORMAT);
        assert_eq!(format_failure.detail, "invalid SQL_CUSTOM boolean");
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
        assert!(Custom::parse("[Default]\nVersion=1\n[q]\nSQL1_1=SELECT 1\nTypo=true").is_err());
        let custom = Custom::parse(
            "[Default]\nVersion=1\n[q]\nSQL1_1=SELECT 1\nPrepared Statement=false\nNumber of Retrys=5\nInput SQF Parser=true\n",
        )
        .unwrap();
        let query = &custom.queries["q"];
        assert!(!query.prepared_statement);
        assert_eq!(query.retries, 5);
        assert!(query.input_sqf_parser);
    }
    #[test]
    fn raw_arrays_stay_arrays_and_strings_cannot_escape() {
        let mut out = sqf::Writer::new(100);
        write_value(&mut out, &Value::from("['rifle',2]"), &[], true).unwrap();
        assert_eq!(out.finish(), "['rifle',2]");
        let mut out = sqf::Writer::new(100);
        write_value(&mut out, &Value::from("[[[any,any,any]]]"), &[], true).unwrap();
        assert_eq!(out.finish(), "[[[any,any,any]]]");
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
