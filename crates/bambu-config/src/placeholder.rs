//! Minimal C++ `PlaceholderParser`: `[key]`, `{expr}`, and `{if}/{elsif}/{else}/{endif}`.

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

    fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }
}

/// Expand macros in `template` using `ctx`. Unknown `{if}` without `{endif}` drops the body.
pub fn expand_placeholders(template: &str, ctx: &PlaceholderContext) -> String {
    expand_range(template, 0, template.len(), ctx).0
}

fn expand_range(s: &str, start: usize, end: usize, ctx: &PlaceholderContext) -> (String, usize) {
    let bytes = s.as_bytes();
    let mut out = String::new();
    let mut i = start;
    while i < end {
        match bytes[i] {
            b'{' => {
                if let Some((brace, next)) = parse_brace(s, i) {
                    match brace {
                        Brace::If(cond) => {
                            let (body, after) = take_if(s, next, &cond, ctx);
                            out.push_str(&body);
                            i = after;
                            continue;
                        }
                        Brace::Expr(inner) => {
                            if let Some(value) = eval_to_string(&inner, ctx) {
                                out.push_str(&value);
                            } else {
                                out.push_str(&s[i..next]);
                            }
                            i = next;
                            continue;
                        }
                        Brace::Elsif(_) | Brace::Else | Brace::Endif => {
                            out.push_str(&s[i..next]);
                            i = next;
                            continue;
                        }
                    }
                }
            }
            b'[' => {
                if let Some(close) = find_closing(bytes, i, b'[', b']') {
                    let inner = s[i + 1..close].trim();
                    if is_ident(inner) {
                        if let Some(value) = ctx.get(inner) {
                            out.push_str(value);
                            i = close + 1;
                            continue;
                        }
                    }
                    out.push_str(&s[i..=close]);
                    i = close + 1;
                    continue;
                }
            }
            _ => {}
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    (out, i)
}

#[derive(Debug)]
enum Brace {
    If(String),
    Elsif(String),
    Else,
    Endif,
    Expr(String),
}

fn parse_brace(s: &str, i: usize) -> Option<(Brace, usize)> {
    let end = find_closing(s.as_bytes(), i, b'{', b'}')?;
    let inner = s[i + 1..end].trim();
    let brace = if inner == "else" {
        Brace::Else
    } else if inner == "endif" {
        Brace::Endif
    } else if let Some(rest) = strip_kw(inner, "elsif") {
        Brace::Elsif(rest.to_string())
    } else if let Some(rest) = strip_kw(inner, "if") {
        Brace::If(rest.to_string())
    } else {
        Brace::Expr(inner.to_string())
    };
    Some((brace, end + 1))
}

fn strip_kw<'a>(inner: &'a str, kw: &str) -> Option<&'a str> {
    let rest = inner.strip_prefix(kw)?;
    if rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == '(') {
        Some(rest.trim())
    } else {
        None
    }
}

fn take_if(s: &str, mut pos: usize, first_cond: &str, ctx: &PlaceholderContext) -> (String, usize) {
    let mut chosen = None;
    let mut take = eval_bool(first_cond, ctx);
    let mut collecting = String::new();
    let mut depth = 0usize;
    let bytes = s.as_bytes();
    while pos < s.len() {
        if bytes[pos] == b'{' {
            if let Some((brace, next)) = parse_brace(s, pos) {
                if depth == 0 {
                    match brace {
                        Brace::Elsif(cond) => {
                            commit_branch(&mut chosen, take, &mut collecting);
                            take = chosen.is_none() && eval_bool(&cond, ctx);
                            pos = next;
                            continue;
                        }
                        Brace::Else => {
                            commit_branch(&mut chosen, take, &mut collecting);
                            take = chosen.is_none();
                            pos = next;
                            continue;
                        }
                        Brace::Endif => {
                            commit_branch(&mut chosen, take, &mut collecting);
                            let body = chosen.unwrap_or_default();
                            return (expand_placeholders(&body, ctx), next);
                        }
                        Brace::If(_) => {
                            depth = 1;
                            collecting.push_str(&s[pos..next]);
                            pos = next;
                            continue;
                        }
                        Brace::Expr(_) => {
                            collecting.push_str(&s[pos..next]);
                            pos = next;
                            continue;
                        }
                    }
                } else {
                    match brace {
                        Brace::If(_) => depth += 1,
                        Brace::Endif => depth -= 1,
                        _ => {}
                    }
                    collecting.push_str(&s[pos..next]);
                    pos = next;
                    continue;
                }
            }
        }
        collecting.push(bytes[pos] as char);
        pos += 1;
    }
    commit_branch(&mut chosen, take, &mut collecting);
    (expand_placeholders(&chosen.unwrap_or_default(), ctx), pos)
}

fn commit_branch(chosen: &mut Option<String>, take: bool, collecting: &mut String) {
    if take && chosen.is_none() {
        *chosen = Some(std::mem::take(collecting));
    } else {
        collecting.clear();
    }
}

fn find_closing(bytes: &[u8], start: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if b == open {
            depth += 1;
        } else if b == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn eval_bool(expr: &str, ctx: &PlaceholderContext) -> bool {
    match eval_value(expr, ctx) {
        Some(Val::Bool(b)) => b,
        Some(Val::Num(n)) => n.abs() > 1e-12,
        Some(Val::Str(s)) => {
            let t = s.trim();
            !t.is_empty() && t != "0" && t != "false" && t != "False"
        }
        None => false,
    }
}

fn eval_to_string(expr: &str, ctx: &PlaceholderContext) -> Option<String> {
    Some(match eval_value(expr, ctx)? {
        Val::Num(n) => fmt_num(n),
        Val::Bool(b) => {
            if b {
                "1".into()
            } else {
                "0".into()
            }
        }
        Val::Str(s) => s,
    })
}

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Num(f64),
    Bool(bool),
    Str(String),
}

impl Val {
    fn as_num(&self) -> Option<f64> {
        match self {
            Val::Num(n) => Some(*n),
            Val::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Val::Str(s) => s.trim().parse().ok(),
        }
    }
}

fn eval_value(expr: &str, ctx: &PlaceholderContext) -> Option<Val> {
    let toks = tokenize(expr)?;
    let mut i = 0;
    let v = parse_or(&toks, &mut i, ctx)?;
    if i != toks.len() {
        return None;
    }
    Some(v)
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Str(String),
    Eq,
    Ne,
    Le,
    Ge,
    Lt,
    Gt,
    And,
    Or,
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    LBrack,
    RBrack,
    Comma,
    Not,
}

fn tokenize(s: &str) -> Option<Vec<Tok>> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'"' {
            i += 1;
            let start = i;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            if i >= b.len() {
                return None;
            }
            out.push(Tok::Str(s[start..i].to_string()));
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit()) {
            let start = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            out.push(Tok::Num(s[start..i].parse().ok()?));
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Tok::Ident(s[start..i].to_string()));
            continue;
        }
        let two = if i + 1 < b.len() {
            Some((c, b[i + 1]))
        } else {
            None
        };
        match two {
            Some((b'=', b'=')) => {
                out.push(Tok::Eq);
                i += 2;
                continue;
            }
            Some((b'!', b'=')) => {
                out.push(Tok::Ne);
                i += 2;
                continue;
            }
            Some((b'<', b'=')) => {
                out.push(Tok::Le);
                i += 2;
                continue;
            }
            Some((b'>', b'=')) => {
                out.push(Tok::Ge);
                i += 2;
                continue;
            }
            Some((b'&', b'&')) => {
                out.push(Tok::And);
                i += 2;
                continue;
            }
            Some((b'|', b'|')) => {
                out.push(Tok::Or);
                i += 2;
                continue;
            }
            _ => {}
        }
        out.push(match c {
            b'<' => Tok::Lt,
            b'>' => Tok::Gt,
            b'+' => Tok::Plus,
            b'-' => Tok::Minus,
            b'*' => Tok::Star,
            b'/' => Tok::Slash,
            b'(' => Tok::LParen,
            b')' => Tok::RParen,
            b'[' => Tok::LBrack,
            b']' => Tok::RBrack,
            b',' => Tok::Comma,
            b'!' => Tok::Not,
            _ => return None,
        });
        i += 1;
    }
    Some(out)
}

fn parse_or(toks: &[Tok], i: &mut usize, ctx: &PlaceholderContext) -> Option<Val> {
    let mut v = parse_and(toks, i, ctx)?;
    while matches!(toks.get(*i), Some(Tok::Or)) {
        *i += 1;
        let r = parse_and(toks, i, ctx)?;
        v = Val::Bool(truthy(&v) || truthy(&r));
    }
    Some(v)
}

fn parse_and(toks: &[Tok], i: &mut usize, ctx: &PlaceholderContext) -> Option<Val> {
    let mut v = parse_cmp(toks, i, ctx)?;
    while matches!(toks.get(*i), Some(Tok::And)) {
        *i += 1;
        let r = parse_cmp(toks, i, ctx)?;
        v = Val::Bool(truthy(&v) && truthy(&r));
    }
    Some(v)
}

fn parse_cmp(toks: &[Tok], i: &mut usize, ctx: &PlaceholderContext) -> Option<Val> {
    let l = parse_add(toks, i, ctx)?;
    let fold = |a: &Val, b: &Val, pred: fn(f64, f64) -> bool, seq: fn(&str, &str) -> bool| {
        if let (Val::Str(x), Val::Str(y)) = (a, b) {
            Val::Bool(seq(x, y))
        } else {
            Val::Bool(pred(a.as_num().unwrap_or(0.0), b.as_num().unwrap_or(0.0)))
        }
    };
    match toks.get(*i) {
        Some(Tok::Eq) => {
            *i += 1;
            let r = parse_add(toks, i, ctx)?;
            Some(fold(&l, &r, |a, b| (a - b).abs() < 1e-9, |a, b| a == b))
        }
        Some(Tok::Ne) => {
            *i += 1;
            let r = parse_add(toks, i, ctx)?;
            Some(fold(&l, &r, |a, b| (a - b).abs() >= 1e-9, |a, b| a != b))
        }
        Some(Tok::Lt) => {
            *i += 1;
            let r = parse_add(toks, i, ctx)?;
            Some(Val::Bool(l.as_num()? < r.as_num()?))
        }
        Some(Tok::Gt) => {
            *i += 1;
            let r = parse_add(toks, i, ctx)?;
            Some(Val::Bool(l.as_num()? > r.as_num()?))
        }
        Some(Tok::Le) => {
            *i += 1;
            let r = parse_add(toks, i, ctx)?;
            Some(Val::Bool(l.as_num()? <= r.as_num()?))
        }
        Some(Tok::Ge) => {
            *i += 1;
            let r = parse_add(toks, i, ctx)?;
            Some(Val::Bool(l.as_num()? >= r.as_num()?))
        }
        _ => Some(l),
    }
}

fn parse_add(toks: &[Tok], i: &mut usize, ctx: &PlaceholderContext) -> Option<Val> {
    let mut v = parse_mul(toks, i, ctx)?;
    loop {
        match toks.get(*i) {
            Some(Tok::Plus) => {
                *i += 1;
                let r = parse_mul(toks, i, ctx)?;
                v = Val::Num(v.as_num()? + r.as_num()?);
            }
            Some(Tok::Minus) => {
                *i += 1;
                let r = parse_mul(toks, i, ctx)?;
                v = Val::Num(v.as_num()? - r.as_num()?);
            }
            _ => return Some(v),
        }
    }
}

fn parse_mul(toks: &[Tok], i: &mut usize, ctx: &PlaceholderContext) -> Option<Val> {
    let mut v = parse_unary(toks, i, ctx)?;
    loop {
        match toks.get(*i) {
            Some(Tok::Star) => {
                *i += 1;
                let r = parse_unary(toks, i, ctx)?;
                v = Val::Num(v.as_num()? * r.as_num()?);
            }
            Some(Tok::Slash) => {
                *i += 1;
                let r = parse_unary(toks, i, ctx)?;
                let d = r.as_num()?;
                if d.abs() < 1e-18 {
                    return None;
                }
                v = Val::Num(v.as_num()? / d);
            }
            _ => return Some(v),
        }
    }
}

fn parse_unary(toks: &[Tok], i: &mut usize, ctx: &PlaceholderContext) -> Option<Val> {
    match toks.get(*i) {
        Some(Tok::Minus) => {
            *i += 1;
            Some(Val::Num(-parse_unary(toks, i, ctx)?.as_num()?))
        }
        Some(Tok::Plus) => {
            *i += 1;
            parse_unary(toks, i, ctx)
        }
        Some(Tok::Not) => {
            *i += 1;
            Some(Val::Bool(!truthy(&parse_unary(toks, i, ctx)?)))
        }
        _ => parse_primary(toks, i, ctx),
    }
}

fn parse_primary(toks: &[Tok], i: &mut usize, ctx: &PlaceholderContext) -> Option<Val> {
    match toks.get(*i).cloned() {
        Some(Tok::Num(n)) => {
            *i += 1;
            Some(Val::Num(n))
        }
        Some(Tok::Str(s)) => {
            *i += 1;
            Some(Val::Str(s))
        }
        Some(Tok::LParen) => {
            *i += 1;
            let v = parse_or(toks, i, ctx)?;
            if !matches!(toks.get(*i), Some(Tok::RParen)) {
                return None;
            }
            *i += 1;
            Some(v)
        }
        Some(Tok::Ident(name)) => {
            *i += 1;
            if name == "max" && matches!(toks.get(*i), Some(Tok::LParen)) {
                *i += 1;
                let a = parse_or(toks, i, ctx)?;
                if !matches!(toks.get(*i), Some(Tok::Comma)) {
                    return None;
                }
                *i += 1;
                let b = parse_or(toks, i, ctx)?;
                if !matches!(toks.get(*i), Some(Tok::RParen)) {
                    return None;
                }
                *i += 1;
                return Some(Val::Num(a.as_num()?.max(b.as_num()?)));
            }
            if matches!(toks.get(*i), Some(Tok::LBrack)) {
                *i += 1;
                let _idx = parse_or(toks, i, ctx)?;
                if !matches!(toks.get(*i), Some(Tok::RBrack)) {
                    return None;
                }
                *i += 1;
            }
            Some(lookup(ctx, &name))
        }
        _ => None,
    }
}

fn lookup(ctx: &PlaceholderContext, name: &str) -> Val {
    match ctx.get(name) {
        Some(raw) => {
            if raw == "true" || raw == "True" {
                Val::Bool(true)
            } else if raw == "false" || raw == "False" {
                Val::Bool(false)
            } else if let Ok(n) = raw.parse::<f64>() {
                Val::Num(n)
            } else {
                Val::Str(raw.to_string())
            }
        }
        None => Val::Num(0.0),
    }
}

fn truthy(v: &Val) -> bool {
    match v {
        Val::Bool(b) => *b,
        Val::Num(n) => n.abs() > 1e-12,
        Val::Str(s) => {
            let t = s.trim();
            !t.is_empty() && t != "0" && t != "false" && t != "False"
        }
    }
}

fn fmt_num(x: f64) -> String {
    if !x.is_finite() {
        return "0".into();
    }
    if (x - x.round()).abs() < 1e-9 {
        format!("{}", x.round() as i64)
    } else {
        let s = format!("{x:.5}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
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
    fn if_else_picks_a_branch() {
        let mut ctx = PlaceholderContext::new();
        ctx.set("long_retraction_when_cut", 1);
        let out = expand_placeholders("{if long_retraction_when_cut}YES{else}NO{endif}", &ctx);
        assert_eq!(out, "YES");
        ctx.set("long_retraction_when_cut", 0);
        let out = expand_placeholders("{if long_retraction_when_cut}YES{else}NO{endif}", &ctx);
        assert_eq!(out, "NO");
    }

    #[test]
    fn nested_z_park_uses_max_layer() {
        let mut ctx = PlaceholderContext::new();
        ctx.set("max_layer_z", 20.2);
        let out = expand_placeholders(
            "{if (100.0 - max_layer_z/2) > 0}\n{if (max_layer_z + 100.0 - max_layer_z/2) < 320}\nG1 Z{max_layer_z + 100.0 - max_layer_z/2} F600\n{else}\nG1 Z320 F600\n{endif}\n{else}\nG1 Z{max_layer_z + 4.0} F600\n{endif}",
            &ctx,
        );
        assert!(out.contains("G1 Z110.1 F600"), "{out}");
        assert!(!out.contains("G1 Z320"), "{out}");
    }

    #[test]
    fn max_of_flush_and_floor() {
        let mut ctx = PlaceholderContext::new();
        ctx.set("flush_volumetric_speeds", 12);
        ctx.set("current_filament_id", 0);
        let out = expand_placeholders(
            "F{max((flush_volumetric_speeds[current_filament_id]/2.4053*60), 200)}",
            &ctx,
        );
        assert!(out.starts_with('F'), "{out}");
        let n: f64 = out[1..].parse().unwrap();
        assert!((n - 12.0 / 2.4053 * 60.0).abs() < 1e-3, "{out}");
    }

    #[test]
    fn elsif_chain() {
        let mut ctx = PlaceholderContext::new();
        ctx.set("bed_temperature", 40);
        let out = expand_placeholders(
            "{if (bed_temperature >45)}HI{elsif(bed_temperature >35)}MID{else}LO{endif}",
            &ctx,
        );
        assert_eq!(out, "MID");
    }
}
