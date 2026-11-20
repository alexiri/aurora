//! (P5-REFINE-7, C-1077, C-1079) What a schedule run or a webhook delivery came to, in words the
//! dashboard shows: derived from the stored outcome/status and the run's turn, never stored.

/// The result of a turn an automation started: `running` while it is active, `answered` when it
/// is done, `failed` (with the turn's error code) when it failed or was stopped. A turn that is
/// gone (its thread was deleted) reads `answered` with reason `thread_deleted`: the run happened
/// and nothing says it failed.
pub fn turn_result(
    turn_status: Option<&str>,
    error_code: Option<&str>,
) -> (&'static str, Option<String>) {
    match turn_status {
        Some("queued" | "dispatched" | "running") => ("running", None),
        Some("done") => ("answered", None),
        Some(s @ ("failed" | "aborted")) => ("failed", Some(error_code.unwrap_or(s).to_owned())),
        Some(_) => ("running", None),
        None => ("answered", Some("thread_deleted".to_owned())),
    }
}

/// `RunResult` and `reason` of a schedule run (the table of C-1077).
pub fn run_result(
    outcome: &str,
    turn_status: Option<&str>,
    error_code: Option<&str>,
) -> (&'static str, Option<String>) {
    let skipped = |why: &str| ("skipped", Some(why.to_owned()));
    match outcome {
        "fired" => turn_result(turn_status, error_code),
        "gated_quiet" => ("skipped_by_gate", None),
        "gate_failed" => ("failed", Some("gate_failed".to_owned())),
        "skipped_unauthorized" => skipped("unauthorized"),
        "skipped_frozen" => skipped("frozen"),
        "coalesced" => skipped("missed"),
        other => skipped(other),
    }
}

/// `WebhookDelivery.result` (C-1079) from the stored status and reason and the delivery's turn.
pub fn delivery_result(
    status: &str,
    reason: Option<&str>,
    turn_status: Option<&str>,
    error_code: Option<&str>,
) -> &'static str {
    match (status, reason) {
        ("accepted", _) => turn_result(turn_status, error_code).0,
        ("duplicate", _) => "duplicate",
        ("rejected", Some("rejected_signature")) => "refused_signature",
        ("rejected", Some("rejected_size")) => "refused_too_large",
        ("paused", Some("frozen")) => "refused_frozen",
        ("paused", _) => "refused_paused",
        _ => "refused_unauthorized",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(o: &str, s: Option<&str>, e: Option<&str>) -> (&'static str, Option<String>) {
        run_result(o, s, e)
    }

    #[test]
    fn every_row_of_the_run_table() {
        for s in ["queued", "dispatched", "running"] {
            assert_eq!(r("fired", Some(s), None), ("running", None));
        }
        assert_eq!(r("fired", Some("done"), None), ("answered", None));
        assert_eq!(
            r("fired", Some("failed"), Some("harness_failed")),
            ("failed", Some("harness_failed".into()))
        );
        assert_eq!(
            r("fired", Some("aborted"), None),
            ("failed", Some("aborted".into()))
        );
        assert_eq!(
            r("fired", None, None),
            ("answered", Some("thread_deleted".into()))
        );
        assert_eq!(r("gated_quiet", None, None), ("skipped_by_gate", None));
        assert_eq!(
            r("gate_failed", Some("failed"), Some("gate_failed")),
            ("failed", Some("gate_failed".into()))
        );
        assert_eq!(
            r("skipped_unauthorized", None, None),
            ("skipped", Some("unauthorized".into()))
        );
        assert_eq!(
            r("skipped_frozen", None, None),
            ("skipped", Some("frozen".into()))
        );
        assert_eq!(
            r("coalesced", None, None),
            ("skipped", Some("missed".into()))
        );
        assert_eq!(
            r("something_new", None, None),
            ("skipped", Some("something_new".into()))
        );
    }

    #[test]
    fn every_delivery_status() {
        let d = delivery_result;
        assert_eq!(
            d("accepted", Some("key:ab"), Some("running"), None),
            "running"
        );
        assert_eq!(
            d("accepted", Some("key:ab"), Some("done"), None),
            "answered"
        );
        assert_eq!(d("accepted", None, Some("failed"), Some("x")), "failed");
        assert_eq!(
            d("duplicate", Some("0191"), Some("done"), None),
            "duplicate"
        );
        assert_eq!(
            d("rejected", Some("rejected_signature"), None, None),
            "refused_signature"
        );
        assert_eq!(
            d("rejected", Some("rejected_size"), None, None),
            "refused_too_large"
        );
        assert_eq!(
            d("rejected", Some("unauthorized"), None, None),
            "refused_unauthorized"
        );
        assert_eq!(d("paused", Some("frozen"), None, None), "refused_frozen");
        assert_eq!(d("paused", Some("disabled"), None, None), "refused_paused");
    }
}
