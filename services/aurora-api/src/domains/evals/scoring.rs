//! Scoring of numeric and text eval questions (docs/10 §Evals, C-862). Pure: no database, no
//! logging of answers or expected values (SEC-116). Judged questions are scored by the judge
//! (`judge.rs`, P4-EVAL-3), outside this pure path; `score` is never asked to score one and
//! answers `judge_not_available` if it is.

use aurora_core::limits;
use aurora_db::repo::evals::Scored;

/// The instruction the platform appends to a numeric or text question (C-862).
pub const ANSWER_INSTRUCTION: &str =
    "End your reply with a final line in exactly this form: ANSWER: <value>";

/// The turn text for a question: the question itself, plus the instruction for the kinds that
/// are scored from the `ANSWER:` line.
pub fn turn_text(question: &str, kind: &str) -> String {
    if kind == "judged" {
        question.to_owned()
    } else {
        format!("{question}\n\n{ANSWER_INSTRUCTION}")
    }
}

/// The value of the last `ANSWER:` line of the reply, trimmed; `None` when there is none.
pub fn extract_answer(reply: &str) -> Option<String> {
    reply.lines().rev().find_map(|line| {
        let l = line.trim().trim_start_matches(['*', '_', '`', '>', ' ']);
        let head = l.get(..7)?;
        if !head.eq_ignore_ascii_case("ANSWER:") {
            return None;
        }
        let v = l[7..].trim().trim_matches(['*', '_', '`']).trim();
        Some(v.to_owned())
    })
}

/// A decimal number: optional sign, digits with optional `,` thousands groups, optional
/// fraction; a trailing full stop is ignored. No exponent, no `inf`/`nan`.
pub fn parse_number(v: &str) -> Option<f64> {
    let t = v.trim().trim_end_matches('.').trim();
    let (sign, rest) = match t.strip_prefix('-') {
        Some(r) => (-1.0, r),
        None => (1.0, t.strip_prefix('+').unwrap_or(t)),
    };
    let (int, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (rest, None),
    };
    let grouped = int.contains(',');
    let int_ok = if grouped {
        let mut parts = int.split(',');
        let first = parts.next().unwrap_or("");
        (1..=3).contains(&first.len())
            && first.bytes().all(|b| b.is_ascii_digit())
            && parts.all(|p| p.len() == 3 && p.bytes().all(|b| b.is_ascii_digit()))
    } else {
        !int.is_empty() && int.bytes().all(|b| b.is_ascii_digit())
    };
    let frac_ok = frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()));
    if !int_ok || !frac_ok {
        return None;
    }
    let clean: String = int.chars().filter(|c| *c != ',').collect();
    let s = match frac {
        Some(f) => format!("{clean}.{f}"),
        None => clean,
    };
    s.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .map(|n| sign * n)
}

/// Text comparison form: trimmed, whitespace collapsed, case-folded.
pub fn normalise_text(v: &str) -> String {
    v.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// An answer as stored: control characters (but tab and newline) removed, cut at
/// `EVAL_ANSWER_CHARS` characters.
pub fn sanitise_answer(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(limits::EVAL_ANSWER_CHARS)
        .collect()
}

/// The expected answer of a result, as snapshotted at run start.
pub struct Expected<'a> {
    pub kind: &'a str,
    pub number: Option<f64>,
    pub tolerance: Option<f64>,
    pub text: Option<&'a str>,
}

/// Score a numeric or text answer. `reply` is the turn's final answer text.
pub fn score(e: &Expected<'_>, reply: &str) -> Scored {
    let answer_text = Some(sanitise_answer(reply));
    if e.kind == "judged" {
        return Scored {
            outcome: "error",
            reason_code: Some("judge_not_available".into()),
            answer_text,
            ..Scored::default()
        };
    }
    let Some(value) = extract_answer(reply) else {
        return Scored {
            outcome: "fail",
            reason_code: Some("no_answer_line".into()),
            answer_text,
            ..Scored::default()
        };
    };
    let extracted_value = Some(sanitise_answer(&value).chars().take(200).collect());
    let (ok, reason) = match e.kind {
        "numeric" => match (parse_number(&value), e.number) {
            (Some(got), Some(want)) => (((got - want).abs() <= e.tolerance.unwrap_or(0.0)), None),
            (None, _) => (false, Some("answer_not_a_number")),
            _ => (false, Some("no_expected_answer")),
        },
        "text" => (
            e.text
                .is_some_and(|t| normalise_text(t) == normalise_text(&value)),
            None,
        ),
        _ => (false, Some("unknown_kind")),
    };
    Scored {
        outcome: if ok { "pass" } else { "fail" },
        reason_code: reason.map(str::to_owned),
        extracted_value,
        answer_text,
        ..Scored::default()
    }
}

/// A question that ended without a scorable answer.
pub fn errored(reason: &str, answer: Option<&str>) -> Scored {
    Scored {
        outcome: "error",
        reason_code: Some(reason.to_owned()),
        answer_text: answer.map(sanitise_answer),
        ..Scored::default()
    }
}

/// A question the run never reached.
pub fn not_scored(reason: &str) -> Scored {
    Scored {
        outcome: "not_scored",
        reason_code: Some(reason.to_owned()),
        ..Scored::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(want: f64, tol: Option<f64>) -> Expected<'static> {
        Expected {
            kind: "numeric",
            number: Some(want),
            tolerance: tol,
            text: None,
        }
    }

    #[test]
    fn answer_line_is_the_last_one() {
        assert_eq!(
            extract_answer("a\nANSWER: 1\nb\nANSWER: 2").as_deref(),
            Some("2")
        );
        assert_eq!(
            extract_answer("thinking\n**ANSWER: 42**").as_deref(),
            Some("42")
        );
        assert_eq!(extract_answer("answer: yes").as_deref(), Some("yes"));
        assert_eq!(extract_answer("the answer is 4"), None);
    }

    #[test]
    fn numbers_exact_and_within_tolerance() {
        assert_eq!(score(&num(42.0, None), "x\nANSWER: 42").outcome, "pass");
        assert_eq!(score(&num(42.0, None), "ANSWER: 42.01").outcome, "fail");
        assert_eq!(score(&num(42.0, Some(0.5)), "ANSWER: 42.4").outcome, "pass");
        assert_eq!(score(&num(42.0, Some(0.5)), "ANSWER: 42.6").outcome, "fail");
        assert_eq!(
            score(&num(1234.5, None), "ANSWER: 1,234.5.").outcome,
            "pass"
        );
        let s = score(&num(1.0, None), "ANSWER: one");
        assert_eq!(
            (s.outcome, s.reason_code.as_deref()),
            ("fail", Some("answer_not_a_number"))
        );
        assert_eq!(parse_number("NaN"), None);
        assert_eq!(parse_number("inf"), None);
        assert_eq!(parse_number("1e3"), None);
        assert_eq!(parse_number("12,34"), None);
    }

    #[test]
    fn text_is_normalised() {
        let e = Expected {
            kind: "text",
            number: None,
            tolerance: None,
            text: Some("  Hello   World "),
        };
        assert_eq!(score(&e, "ANSWER: hello world").outcome, "pass");
        assert_eq!(score(&e, "ANSWER: hello there").outcome, "fail");
    }

    #[test]
    fn missing_answer_line_fails_with_reason() {
        let s = score(&num(1.0, None), "I think it is 1");
        assert_eq!(
            (s.outcome, s.reason_code.as_deref()),
            ("fail", Some("no_answer_line"))
        );
    }

    #[test]
    fn answers_are_cut_and_cleaned() {
        let long = "a".repeat(limits::EVAL_ANSWER_CHARS + 50);
        assert_eq!(
            sanitise_answer(&long).chars().count(),
            limits::EVAL_ANSWER_CHARS
        );
        assert_eq!(sanitise_answer("a\u{0}b\u{1b}c\nd"), "abc\nd");
    }
}
