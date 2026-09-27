//! A small JSON reader for the cells (journal lines, `--status-fd`, `--json`): enough to check
//! that a line parses and to read its fields. Strict: one value, then only whitespace.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn get(&self, k: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.get(k),
            _ => None,
        }
    }
    pub fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn num(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
}

pub fn parse(s: &str) -> Result<Json, String> {
    let b = s.as_bytes();
    let mut i = 0;
    let v = value(b, &mut i)?;
    ws(b, &mut i);
    if i != b.len() {
        return Err(format!("trailing bytes at {i}"));
    }
    Ok(v)
}

fn ws(b: &[u8], i: &mut usize) {
    while *i < b.len() && b" \t\r\n".contains(&b[*i]) {
        *i += 1;
    }
}

fn value(b: &[u8], i: &mut usize) -> Result<Json, String> {
    ws(b, i);
    match b.get(*i) {
        Some(b'{') => {
            *i += 1;
            let mut m = BTreeMap::new();
            ws(b, i);
            if b.get(*i) == Some(&b'}') {
                *i += 1;
                return Ok(Json::Obj(m));
            }
            loop {
                ws(b, i);
                let Json::Str(k) = string(b, i)? else { unreachable!() };
                ws(b, i);
                if b.get(*i) != Some(&b':') {
                    return Err(format!("expected : at {i}"));
                }
                *i += 1;
                let v = value(b, i)?;
                m.insert(k, v);
                ws(b, i);
                match b.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b'}') => {
                        *i += 1;
                        return Ok(Json::Obj(m));
                    }
                    _ => return Err(format!("expected , or }} at {i}")),
                }
            }
        }
        Some(b'[') => {
            *i += 1;
            let mut a = Vec::new();
            ws(b, i);
            if b.get(*i) == Some(&b']') {
                *i += 1;
                return Ok(Json::Arr(a));
            }
            loop {
                a.push(value(b, i)?);
                ws(b, i);
                match b.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b']') => {
                        *i += 1;
                        return Ok(Json::Arr(a));
                    }
                    _ => return Err(format!("expected , or ] at {i}")),
                }
            }
        }
        Some(b'"') => string(b, i),
        Some(b't') if b[*i..].starts_with(b"true") => {
            *i += 4;
            Ok(Json::Bool(true))
        }
        Some(b'f') if b[*i..].starts_with(b"false") => {
            *i += 5;
            Ok(Json::Bool(false))
        }
        Some(b'n') if b[*i..].starts_with(b"null") => {
            *i += 4;
            Ok(Json::Null)
        }
        Some(c) if *c == b'-' || c.is_ascii_digit() => {
            let s = *i;
            *i += 1;
            while *i < b.len() && (b[*i].is_ascii_digit() || b"+-.eE".contains(&b[*i])) {
                *i += 1;
            }
            std::str::from_utf8(&b[s..*i]).unwrap().parse().map(Json::Num).map_err(|e| e.to_string())
        }
        _ => Err(format!("unexpected byte at {i}")),
    }
}

fn string(b: &[u8], i: &mut usize) -> Result<Json, String> {
    if b.get(*i) != Some(&b'"') {
        return Err(format!("expected a string at {i}"));
    }
    *i += 1;
    let mut out = String::new();
    loop {
        let c = *b.get(*i).ok_or("unterminated string")?;
        *i += 1;
        match c {
            b'"' => return Ok(Json::Str(out)),
            b'\\' => {
                let e = *b.get(*i).ok_or("unterminated escape")?;
                *i += 1;
                match e {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let h = std::str::from_utf8(b.get(*i..*i + 4).ok_or("short \\u")?).map_err(|e| e.to_string())?;
                        let n = u32::from_str_radix(h, 16).map_err(|e| e.to_string())?;
                        out.push(char::from_u32(n).ok_or("bad \\u")?);
                        *i += 4;
                    }
                    _ => return Err(format!("bad escape at {i}")),
                }
            }
            c if c < 0x20 => return Err(format!("control byte in a string at {i}")),
            _ => {
                // copy one UTF-8 sequence
                let s = *i - 1;
                let len = match c {
                    0x00..=0x7f => 1,
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    _ => 4,
                };
                let chunk = std::str::from_utf8(b.get(s..s + len).ok_or("short utf-8")?).map_err(|e| e.to_string())?;
                out.push_str(chunk);
                *i = s + len;
            }
        }
    }
}
