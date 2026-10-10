use mysql_async::Value;

pub struct Writer {
    text: String,
    limit: usize,
}
impl Writer {
    pub fn new(limit: usize) -> Self {
        Self {
            text: String::new(),
            limit,
        }
    }
    pub fn push(&mut self, text: &str) -> Result<(), String> {
        if text.len() > self.limit.saturating_sub(self.text.len()) {
            return Err("response exceeds configured byte limit".into());
        }
        self.text.push_str(text);
        Ok(())
    }
    pub fn prepend(&mut self, text: &str) -> Result<(), String> {
        if text.len() > self.limit.saturating_sub(self.text.len()) {
            return Err("response exceeds configured byte limit".into());
        }
        self.text.insert_str(0, text);
        Ok(())
    }
    pub fn quoted(&mut self, text: &str) -> Result<(), String> {
        self.push("\"")?;
        for chunk in text.split_inclusive('"') {
            self.push(chunk)?;
            if chunk.ends_with('"') {
                self.push("\"")?;
            }
        }
        self.push("\"")
    }
    pub fn finish(self) -> String {
        self.text
    }
    pub fn remaining(&self) -> usize {
        self.limit.saturating_sub(self.text.len())
    }
}

pub fn literal(text: &str) -> bool {
    struct Parser<'a> {
        bytes: &'a [u8],
        pos: usize,
    }
    impl Parser<'_> {
        fn ws(&mut self) {
            while self
                .bytes
                .get(self.pos)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.pos += 1;
            }
        }
        fn value(&mut self, depth: usize) -> bool {
            if depth > 64 {
                return false;
            }
            self.ws();
            match self.bytes.get(self.pos).copied() {
                Some(b'[') => {
                    self.pos += 1;
                    self.ws();
                    if self.bytes.get(self.pos) == Some(&b']') {
                        self.pos += 1;
                        return true;
                    }
                    loop {
                        if !self.value(depth + 1) {
                            return false;
                        }
                        self.ws();
                        match self.bytes.get(self.pos) {
                            Some(b',') => self.pos += 1,
                            Some(b']') => {
                                self.pos += 1;
                                return true;
                            }
                            _ => return false,
                        }
                    }
                }
                Some(quote @ (b'\'' | b'"')) => {
                    self.pos += 1;
                    while let Some(&ch) = self.bytes.get(self.pos) {
                        self.pos += 1;
                        if ch == 0 {
                            return false;
                        }
                        if ch == quote {
                            if self.bytes.get(self.pos) == Some(&quote) {
                                self.pos += 1;
                            } else {
                                return true;
                            }
                        }
                    }
                    false
                }
                Some(_) => {
                    let start = self.pos;
                    while self
                        .bytes
                        .get(self.pos)
                        .is_some_and(|b| !b.is_ascii_whitespace() && *b != b',' && *b != b']')
                    {
                        self.pos += 1;
                    }
                    let token = &self.bytes[start..self.pos];
                    token.eq_ignore_ascii_case(b"true")
                        || token.eq_ignore_ascii_case(b"false")
                        || token.eq_ignore_ascii_case(b"objnull")
                        || number(token)
                }
                None => false,
            }
        }
    }
    let mut parser = Parser {
        bytes: text.as_bytes(),
        pos: 0,
    };
    let valid = parser.value(0);
    parser.ws();
    valid && parser.pos == parser.bytes.len()
}

fn number(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    if bytes.is_empty()
        || !bytes
            .iter()
            .all(|b| b.is_ascii_digit() || b"+-.eE".contains(b))
    {
        return false;
    }
    text.parse::<f64>().is_ok_and(f64::is_finite)
}

pub fn text(value: &Value) -> Result<String, String> {
    Ok(match value {
        Value::NULL => String::new(),
        Value::Bytes(bytes) => std::str::from_utf8(bytes)
            .map_err(|_| "database value is not UTF-8")?
            .to_owned(),
        Value::Int(v) => v.to_string(),
        Value::UInt(v) => v.to_string(),
        Value::Float(v) if v.is_finite() => format!("{v:.6}"),
        Value::Double(v) if v.is_finite() => format!("{v:.6}"),
        Value::Float(_) | Value::Double(_) => return Err("non-finite database number".into()),
        Value::Date(y, m, d, h, n, s, us) => format!("[{y},{m},{d},{h},{n},{}]", seconds(*s, *us)),
        Value::Time(negative, days, h, n, s, us) => {
            if *negative {
                return Err("negative SQL TIME has no supported SQF date representation".into());
            }
            format!("[0,0,{days},{h},{n},{}]", seconds(*s, *us))
        }
    })
}
fn seconds(s: u8, us: u32) -> String {
    if us == 0 {
        s.to_string()
    } else {
        format!("{s}.{us:06}").trim_end_matches('0').to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_data_rejects_code() {
        for value in ["[]", "['a',\"b\"\"c\",[-1,1e-3,true,objNull]]", " 42 "] {
            assert!(literal(value), "{value}");
        }
        for value in [
            "call evil",
            "[];call evil",
            "[1+2]",
            "[player]",
            "[nil]",
            "NaN",
            "1e999",
            "[1,]",
            "\"x\" call evil",
        ] {
            assert!(!literal(value), "{value}");
        }
        assert!(!literal(&format!("{}0{}", "[".repeat(66), "]".repeat(66))));
    }
    #[test]
    fn quotes_and_budget_are_exact() {
        let mut out = Writer::new(8);
        out.quoted("a\"b").unwrap();
        assert_eq!(out.finish(), "\"a\"\"b\"");
        let mut out = Writer::new(5);
        assert!(out.quoted("a\"b").is_err());
    }
    #[test]
    fn prepend_obeys_the_same_response_byte_limit() {
        let mut out = Writer::new(4);
        out.push("xyz").unwrap();
        out.prepend("a").unwrap();
        assert!(out.push("!").is_err());
        assert_eq!(out.finish(), "axyz");
    }
    #[test]
    fn numbers_and_dates() {
        assert_eq!(
            text(&Value::UInt(u64::MAX)).unwrap(),
            "18446744073709551615"
        );
        assert_eq!(
            text(&Value::Date(2026, 10, 10, 12, 3, 4, 120000)).unwrap(),
            "[2026,10,10,12,3,4.12]"
        );
        assert_eq!(text(&Value::Float(0.75)).unwrap(), "0.750000");
        assert_eq!(text(&Value::Double(0.1)).unwrap(), "0.100000");
        assert!(text(&Value::Double(f64::INFINITY)).is_err());
    }
}
