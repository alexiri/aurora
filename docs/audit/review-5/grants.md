# REVIEW-5 findings — grants (saved by the coordinator from the reviewer's hand-back)

## GRANT-1 (high, security, CONFIRMED) — a held request is forwarded after approval with the credential chosen before the hold; grant revocation / restricting activation / policy change in between is not re-checked
- `services/aurora-gateway/src/decide.rs:665-671`, `:822-835`, `:876-900`; `hold.rs:382`; api `approvals/service.rs:63-109`; `grants/service.rs:721-745` (`revoke_connection` doesn't abandon pending approvals).
- Spec: docs/07 §Lifecycle (~470) covers freeze only ("marks its pending approvals abandoned") — silent on revoke/disable/policy change → spec gap.
- Fix: abandon affected pending approvals in the same tx as grant delete / connection disable / restricting activation (freeze precedent), and/or re-evaluate + re-read grant after `Approved`. State the rule in docs/07.

## GRANT-2 (high, PLAUSIBLE: matcher confirmed, upstream parser not) — `;` is an ordinary value char; `Action=DescribeInstances;Action=TerminateInstances` passes a `prefix Describe` read tool
- `crates/aurora-policy/src/params.rs:282-292`, `:295-325`, `:266-279`; `plugins/builtin/aws-keys/plugin.yaml`, `aws-role/plugin.yaml` (`ec2_describe`). SEC-68, D-124.
- Fix: raw `;` in query/form = doubt (refuse). Corpus case in `tests/params.yaml`.

## GRANT-3 (medium) — `custom_only` does not imply `block_untooled`; a hand-written `network` allow gives untooled requests the connection's credential
- `crates/aurora-registry/src/manifest/model.rs:697-704`; `validate/params_rules.rs:95-101`; `services/aurora-gateway/src/apps.rs:85-96`; `crates/aurora-policy/src/eval.rs:239-249`; `rule.rs:112-120`, `:161-170`. docs/09:437-442; docs/07 §App-level restrictions; FIX-064.md:43 (open owner question on network targets).

## GRANT-4 (medium) — params-vs-params shadowing not checked; a write params tool can be unreachable behind a read one
- `validate/params_rules.rs:70-72`, `:95-135`. docs/09 §6 `params_tool_shadowed`.

## GRANT-5 (medium, PLAUSIBLE) — parameter names compared case-sensitively (`ACTION=AssumeRole&Action=GetCallerIdentity`)
- `params.rs:266-279` vs `header_once` `:198-210`.

## GRANT-6 (low) — library fetch uses the connection without the app's `grants` flags (`library_fetch.rs:325`; cf. `resolve.rs:524-526`, `:739`; docs/06 ~43 vs docs/09, SEC-67)
## GRANT-7 (low) — `form` condition on a tool with no `methods` passes the linter (`params_rules.rs:87-89`, `:116`; `apps.rs:275-277`)
## GRANT-8 (low, PLAUSIBLE) — `json_pointer` conditions use the last duplicate JSON key (`rule.rs:332-343`)
## GRANT-9 (low, test-gap) — linter's `positive` fixture satisfied by a claim, not a match (`params_rules.rs:310-312`)

## Not covered
- Indico/GitHub/GitLab tool tables; aws-role hook minting; mixed-version gateways; snapshot activation across gateways; upstream parser behaviour; approval summary truncation vs condition_body_limit.
