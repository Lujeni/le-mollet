//! The audit trail: one logfmt line on stderr per account event (sign-up, sign-in, link, settings, deletion,
//! roles), which journald dates and keeps on a server. `journalctl -u le-mollet -g '^audit '`, or
//! `-g 'account=1234'` for one account. E-mails and addresses are in it as such: it is the operator's log.

/// `audit event=<event> k=v …`; a value with a space, a quote or nothing in it is quoted.
pub fn audit(event: &str, fields: &[(&str, &str)]) {
    let mut line = format!("audit event={event}");
    for (k, v) in fields {
        let v = if v.is_empty() || v.contains([' ', '"', '=']) { format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")) } else { (*v).to_owned() };
        line.push_str(&format!(" {k}={v}"));
    }
    eprintln!("{line}");
}

#[cfg(test)]
mod tests {
    #[test]
    fn quotes_what_needs_it() {
        let v = |s: &str| if s.is_empty() || s.contains([' ', '"', '=']) { format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")) } else { s.to_owned() };
        assert_eq!(v("paul"), "paul");
        assert_eq!(v("wrong name or password"), "\"wrong name or password\"");
        assert_eq!(v("a\"b"), "\"a\\\"b\"");
        assert_eq!(v(""), "\"\"");
    }
}
