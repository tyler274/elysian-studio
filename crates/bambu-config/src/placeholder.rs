//! Minimal C++ `PlaceholderParser`: `[key]`, `{key}`, and `{key+n}`.

use std::collections::BTreeMap;

#[derive(Debug, Default, Clone)]
pub struct PlaceholderContext {
    vars: BTreeMap<String, String>,
}

impl PlaceholderContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl ToString) {
        self.vars.insert(key.into(), value.to_string());
    }
}

/// Expand `[ident]` and `{ident}` / `{ident+n}` using `ctx`. Unknown braces are kept.
pub fn expand_placeholders(template: &str, ctx: &PlaceholderContext) -> String {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                if let Some(end) = find_closing(bytes, i, b'{', b'}') {
                    let inner = &template[i + 1..end];
                    if let Some(value) = eval_expr(inner, ctx) {
                        out.push_str(&value);
                    } else {
                        out.push_str(&template[i..=end]);
                    }
                    i = end + 1;
                    continue;
                }
            }
            b'[' => {
                if let Some(end) = find_closing(bytes, i, b'[', b']') {
                    let inner = template[i + 1..end].trim();
                    if is_ident(inner) {
                        if let Some(value) = ctx.vars.get(inner) {
                            out.push_str(value);
                            i = end + 1;
                            continue;
                        }
                    }
                    out.push_str(&template[i..=end]);
                    i = end + 1;
                    continue;
                }
            }
            _ => {}
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn find_closing(bytes: &[u8], start: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn eval_expr(inner: &str, ctx: &PlaceholderContext) -> Option<String> {
    let s = inner.trim();
    if s.is_empty()
        || s.starts_with("if")
        || s.contains("==")
        || s.contains("&&")
        || s.contains("||")
        || s.contains('(')
    {
        return None;
    }
    let (name, delta) = parse_ref(s)?;
    let raw = ctx.vars.get(name)?;
    apply_delta(raw, delta)
}

fn parse_ref(s: &str) -> Option<(&str, f64)> {
    let bytes = s.as_bytes();
    for i in (1..bytes.len()).rev() {
        if bytes[i] == b'+' || bytes[i] == b'-' {
            let name = s[..i].trim();
            let rest = s[i..].trim();
            if is_ident(name) {
                let delta: f64 = rest.parse().ok()?;
                return Some((name, delta));
            }
        }
    }
    is_ident(s.trim()).then_some((s.trim(), 0.0))
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

fn apply_delta(raw: &str, delta: f64) -> Option<String> {
    if delta.abs() < 1e-12 {
        return Some(raw.to_string());
    }
    if let Ok(n) = raw.parse::<i64>() {
        if (delta - delta.round()).abs() < 1e-9 {
            return Some((n + delta.round() as i64).to_string());
        }
        return Some((n as f64 + delta).to_string());
    }
    let x: f64 = raw.parse().ok()?;
    let y = x + delta;
    if (y - y.round()).abs() < 1e-9 {
        Some((y.round() as i64).to_string())
    } else {
        Some(y.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_layer_change_template() {
        let mut ctx = PlaceholderContext::new();
        ctx.set("layer_num", 0);
        ctx.set("total_layer_count", 42);
        let out = expand_placeholders(
            "; layer num/total_layer_count: {layer_num+1}/[total_layer_count]\nM73 L{layer_num+1}\nM991 S0 P{layer_num} ;notify layer change",
            &ctx,
        );
        assert!(out.contains("1/42"), "{out}");
        assert!(out.contains("M73 L1"), "{out}");
        assert!(out.contains("M991 S0 P0"), "{out}");
    }

    #[test]
    fn keeps_unknown_braces() {
        let ctx = PlaceholderContext::new();
        let out = expand_placeholders("{if foo}keep", &ctx);
        assert_eq!(out, "{if foo}keep");
    }
}
