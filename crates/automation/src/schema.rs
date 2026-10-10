//! JSON Schema for a command's parameters, read from its parameter doc (`CommandSpec::params`, the
//! short notation `command_list` shows: `{"clips":[id]?,"target":"speaker|group|face"?,"zoom":100-400?,
//! "wait":bool=false}`), so MCP clients get typed parameters instead of a string to interpret.
//!
//! The notation is informal, so the schema is a best effort and stays permissive: an unknown type
//! accepts anything, and extra properties are allowed (the engine validates every call).

use serde_json::{Map, Value, json};

/// The schema of a parameter doc; `{"type":"object"}` when the doc is not an object.
pub fn params_schema(doc: &str) -> Value {
    let doc = doc.trim();
    match split_object(doc) {
        Some(fields) => {
            let mut props = Map::new();
            let mut required = Vec::new();
            for (name, ty) in fields {
                let (schema, optional) = field(&ty);
                if !optional {
                    required.push(Value::String(name.clone()));
                }
                props.insert(name, schema);
            }
            let mut o = json!({"type": "object", "properties": props, "additionalProperties": true});
            if !required.is_empty() {
                o["required"] = Value::Array(required);
            }
            o
        }
        None => json!({"type": "object", "additionalProperties": true}),
    }
}

/// `{"a":T,"b":U?}` → [(a, T), (b, U?)]: top-level fields, nesting and quotes respected.
fn split_object(s: &str) -> Option<Vec<(String, String)>> {
    let inner = s.strip_prefix('{')?.strip_suffix('}')?;
    let mut out = Vec::new();
    for part in split_top(inner, ',') {
        let part = part.trim();
        if part.is_empty() || part.starts_with('…') || part.starts_with("...") {
            continue;
        }
        let (name, ty) = match part.strip_prefix('"') {
            Some(rest) => {
                let end = rest.find('"')?;
                let ty = rest[end + 1..].trim_start().strip_prefix(':')?;
                (rest[..end].to_string(), ty.trim().to_string())
            }
            None => continue,
        };
        out.push((name, ty));
    }
    Some(out)
}

/// Split at `sep` outside quotes, brackets and braces.
fn split_top(s: &str, sep: char) -> Vec<String> {
    let (mut out, mut cur, mut depth, mut quoted) = (Vec::new(), String::new(), 0i32, false);
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            '[' | '{' | '(' if !quoted => depth += 1,
            ']' | '}' | ')' if !quoted => depth -= 1,
            _ => {}
        }
        if c == sep && depth == 0 && !quoted {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// A field's type notation → (schema, optional).
fn field(ty: &str) -> (Value, bool) {
    let mut t = ty.trim();
    let mut optional = false;
    let mut default = None;
    if let Some(x) = t.strip_suffix('?') {
        t = x.trim_end();
        optional = true;
    }
    // `T=default` (outside quotes and brackets)
    let parts = split_top(t, '=');
    if parts.len() == 2 {
        t = parts[0].trim();
        let d = parts[1].trim();
        default = Some(serde_json::from_str::<Value>(d).unwrap_or_else(|_| Value::String(d.to_string())));
        optional = true;
    }
    let t = t.to_string();
    let mut s = type_schema(&t);
    if let (Some(d), Some(o)) = (default, s.as_object_mut()) {
        o.insert("default".into(), d);
    }
    (s, optional)
}

fn type_schema(t: &str) -> Value {
    let t = t.trim();
    // alternatives at the top level: `id|"C1"`, `"a"|"b"`, `ticks?|"frames"`
    let alts = split_top(t, '|');
    if alts.len() > 1 && !(t.starts_with('"') && t.ends_with('"') && !t[1..t.len() - 1].contains('"')) {
        let schemas: Vec<Value> = alts.iter().map(|a| type_schema(a)).collect();
        // all string literals: one enum
        if schemas.iter().all(|s| s.get("const").is_some()) {
            return json!({"enum": schemas.iter().map(|s| s["const"].clone()).collect::<Vec<_>>()});
        }
        return json!({"anyOf": schemas});
    }
    // a quoted string: `"a|b|c"` is an enum, `"#rrggbb"` or `"name"` a string, `"x"` a constant
    if let Some(inner) = t.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
        if inner.contains('|') {
            let opts: Vec<&str> = inner.split('|').map(str::trim).filter(|o| !o.is_empty() && !o.contains('…')).collect();
            return json!({"type": "string", "enum": opts});
        }
        if inner.starts_with('#') || inner.contains(' ') || inner.is_empty() {
            return json!({"type": "string", "description": inner});
        }
        return json!({"const": inner});
    }
    if let Some(inner) = t.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
        let item = inner.trim();
        return if item.is_empty() || item.contains('…') { json!({"type": "array"}) } else { json!({"type": "array", "items": type_schema(item)}) };
    }
    if t.starts_with('{') && t.ends_with('}') {
        return match split_object(t) {
            Some(f) if !f.is_empty() => params_schema(t),
            _ => json!({"type": "object"}),
        };
    }
    // a range of numbers: `0-1`, `1-20`, `100-400`, `0..100`, `≤600`
    // (a number: `0-1` is a fraction, `1-20` a count; integers are numbers too)
    if let Some((lo, hi)) = t.split_once("..").or_else(|| t.split_once('-').filter(|(a, _)| !a.is_empty()))
        && let (Ok(lo), Ok(hi)) = (lo.trim().parse::<f64>(), hi.trim().parse::<f64>())
    {
        return json!({"type": "number", "minimum": lo, "maximum": hi});
    }
    if let Some(hi) = t.strip_prefix('≤').and_then(|x| x.trim().parse::<f64>().ok()) {
        return json!({"type": "number", "maximum": hi});
    }
    match t {
        "bool" => json!({"type": "boolean"}),
        "str" | "string" | "path" | "timecode" | "lang" => json!({"type": "string"}),
        "f64" | "f32" | "number" | "seconds" | "db" => json!({"type": "number"}),
        "id" | "u32" | "u64" | "i64" | "i32" | "n" | "frames" | "ticks" | "index" | "frame" | "int" | "usize" => json!({"type": "integer"}),
        _ => json!({"description": t}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parameter_doc_becomes_a_schema() {
        let s = params_schema(r#"{"clips":[id]?,"target":"speaker|group|face"?,"x":0-1?,"zoom":100-400?,"wait":bool=false,"path":str,"track":id|"C1"?}"#);
        assert_eq!(s["type"], "object");
        assert_eq!(s["required"], json!(["path"]));
        let p = &s["properties"];
        assert_eq!(p["clips"], json!({"type": "array", "items": {"type": "integer"}}));
        assert_eq!(p["target"], json!({"type": "string", "enum": ["speaker", "group", "face"]}));
        assert_eq!(p["x"], json!({"type": "number", "minimum": 0.0, "maximum": 1.0}));
        assert_eq!(p["zoom"]["maximum"], 400.0);
        assert_eq!(p["wait"], json!({"type": "boolean", "default": false}));
        assert_eq!(p["path"], json!({"type": "string"}));
        assert_eq!(p["track"]["anyOf"][0], json!({"type": "integer"}));
        assert_eq!(p["track"]["anyOf"][1], json!({"const": "C1"}));
    }

    #[test]
    fn odd_or_hostile_docs_never_fail() {
        for doc in ["{}", "", "not a doc", "{\"a\"", "{\"a\":}", "{…settings params,\"path\":str|dir?}", r#"{"x":{"y":[[id]]}?}"#, "{\"a\":\"\"}"] {
            let s = params_schema(doc);
            assert_eq!(s["type"], "object", "{doc}");
        }
        assert_eq!(params_schema(r#"{"mix":"Stereo|Mono|5.1"?}"#)["properties"]["mix"]["enum"], json!(["Stereo", "Mono", "5.1"]));
        assert_eq!(params_schema(r#"{"ws":"rec709"|"rec2100-pq"?}"#)["properties"]["ws"]["enum"], json!(["rec709", "rec2100-pq"]));
    }

    /// Every command of the engine gets a schema of an object.
    #[test]
    fn every_engine_command_has_a_schema() {
        for c in filmcraft_engine::command_specs() {
            let s = params_schema(c.params);
            assert_eq!(s["type"], "object", "{}: {}", c.id, c.params);
        }
    }
}
