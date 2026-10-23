const MATH_FUNCTIONS: [&str; 9] = ["sqrt", "cbrt", "abs", "pow", "ln", "log10", "log2", "exp", "hypot"];

pub fn calculate(expression: &str) -> anyhow::Result<String> {
    let expr = expression.trim();
    anyhow::ensure!(!expr.is_empty(), crate::prompts::render("errors/empty_expression.j2", ()));
    let mut code = expr.replace("math::", "");
    for f in MATH_FUNCTIONS {
        code = code.replace(&format!("{f}("), &format!("math::{f}("));
    }
    let value = evalexpr::eval(&code).map_err(|e| anyhow::anyhow!(crate::prompts::render("errors/cannot_evaluate.j2", minijinja::context! { expr, error => e.to_string() })))?;
    Ok(match value {
        evalexpr::Value::Int(i) => i.to_string(),
        evalexpr::Value::Float(f) if f.is_finite() => {
            let s = format!("{:.10}", f);
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        }
        evalexpr::Value::Float(f) => anyhow::bail!("result is {f}"),
        evalexpr::Value::Boolean(b) => b.to_string(),
        other => anyhow::bail!("not a number: {other}"),
    })
}

pub fn parse_time(text: &str) -> Option<f64> {
    let t = text.trim().trim_matches(|c| matches!(c, '[' | ']' | '@' | ' ')).replace(',', ".");
    if t.is_empty() {
        return None;
    }
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0;
    for (i, part) in parts.iter().enumerate() {
        let v: f64 = part.trim().parse().ok().filter(|v: &f64| v.is_finite() && *v >= 0.0)?;
        if i > 0 && v >= 60.0 {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

pub fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0).floor() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn between(from: &str, to: &str) -> anyhow::Result<(f64, String)> {
    let a = parse_time(from).ok_or_else(|| anyhow::anyhow!(crate::prompts::render("errors/not_a_clock_time.j2", minijinja::context! { time => from })))?;
    let b = parse_time(to).ok_or_else(|| anyhow::anyhow!(crate::prompts::render("errors/not_a_clock_time.j2", minijinja::context! { time => to })))?;
    let d = (b - a).abs();
    Ok((d, format_time(d)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_is_exact() {
        assert_eq!(calculate("2 + 2 * 2").unwrap(), "6");
        assert_eq!(calculate("(754.5 - 359) / 60").unwrap(), "6.5916666667");
        assert_eq!(calculate("0.1 + 0.2").unwrap(), "0.3", "no float noise");
        assert_eq!(calculate("17 % 5").unwrap(), "2");
        assert_eq!(calculate("2 ^ 10").unwrap(), "1024");
        assert_eq!(calculate("math::sqrt(16.0)").unwrap(), "4");
        assert_eq!(calculate("sqrt(2.25) + abs(-1)").unwrap(), "2.5", "without the namespace too");
        assert!(calculate("").is_err());
        assert!(calculate("2 +").is_err());
        assert!(calculate("1 / 0.0").is_err(), "infinity is reported, not returned");
    }

    #[test]
    fn times_as_written_in_transcripts() {
        assert_eq!(parse_time("5:59"), Some(359.0));
        assert_eq!(parse_time("1:02:30"), Some(3750.0));
        assert_eq!(parse_time("[12:30"), Some(750.0));
        assert_eq!(parse_time("@5:59"), Some(359.0));
        assert_eq!(parse_time("75.5"), Some(75.5));
        assert_eq!(parse_time("1:05,5"), Some(65.5));
        assert_eq!(parse_time("5:75"), None, "not a clock time");
        assert_eq!(parse_time("1:2:3:4"), None);
        assert_eq!(parse_time("soon"), None);
        assert_eq!(parse_time("-3"), None);
        assert_eq!(format_time(359.9), "5:59");
        assert_eq!(format_time(3750.0), "1:02:30");
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(between("5:59", "12:30").unwrap(), (391.0, "6:31".to_string()));
        assert_eq!(between("1:00:00", "59:00").unwrap().0, 60.0, "either order");
        assert!(between("x", "1:00").unwrap_err().to_string().contains("not a time"));
    }
}
