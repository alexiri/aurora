//! Registry of every protocol JSON Schema. Each entry is written to
//! `schemas/<name>.json` at the repo root; `tests/schemas.rs` fails on drift.

use schemars::{Schema, schema_for};

use crate::runner as r;
use crate::supervisor as s;
use crate::tools as t;
use crate::version::ProtocolVersion;
use crate::vocab::ExampleKind;

/// All registered schemas as `(file stem, schema)` pairs.
pub fn all() -> Vec<(&'static str, Schema)> {
    vec![
        ("protocol_version", schema_for!(ProtocolVersion)),
        ("example_vocab", schema_for!(ExampleKind)),
        // W5
        ("runner_register_request", schema_for!(r::RegisterRequest)),
        ("runner_register_response", schema_for!(r::RegisterResponse)),
        ("runner_work_request", schema_for!(r::WorkRequest)),
        ("runner_work_response", schema_for!(r::WorkResponse)),
        ("runner_work_item", schema_for!(r::WorkItem)),
        ("runner_report", schema_for!(r::Report)),
        ("runner_reports_request", schema_for!(r::ReportsRequest)),
        ("runner_reports_response", schema_for!(r::ReportsResponse)),
        (
            "runner_channel_claim_request",
            schema_for!(r::ChannelClaimRequest),
        ),
        (
            "runner_channel_claim_response",
            schema_for!(r::ChannelClaimResponse),
        ),
        ("runner_channel_ref", schema_for!(r::ChannelRef)),
        ("runner_ok_response", schema_for!(r::OkResponse)),
        (
            "runner_memory_write_request",
            schema_for!(r::MemoryWriteRequest),
        ),
        (
            "runner_memory_write_response",
            schema_for!(r::MemoryWriteResponse),
        ),
        (
            "runner_attachment_upload_meta",
            schema_for!(r::AttachmentUploadMeta),
        ),
        (
            "runner_attachment_upload_response",
            schema_for!(r::AttachmentUploadResponse),
        ),
        (
            "runner_sandboxes_response",
            schema_for!(r::SandboxesResponse),
        ),
        (
            "runner_sandboxes_check_request",
            schema_for!(r::SandboxesCheckRequest),
        ),
        (
            "runner_sandboxes_check_response",
            schema_for!(r::SandboxesCheckResponse),
        ),
        ("runner_strays_request", schema_for!(r::StraysRequest)),
        // W6
        ("supervisor_hello", schema_for!(s::Hello)),
        ("supervisor_welcome", schema_for!(s::Welcome)),
        (
            "supervisor_frame_runner_to_supervisor",
            schema_for!(s::Frame<s::RunnerToSupervisor>),
        ),
        (
            "supervisor_frame_supervisor_to_runner",
            schema_for!(s::Frame<s::SupervisorToRunner>),
        ),
        // W7
        ("tools_tool_call", schema_for!(t::ToolCall)),
        ("tools_tool_result", schema_for!(t::ToolResult)),
        ("tools_tool_descriptor", schema_for!(t::ToolDescriptor)),
    ]
}
