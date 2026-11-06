//! Registry of every protocol JSON Schema. Each entry is written to
//! `schemas/<name>.json` at the repo root; `tests/schemas.rs` fails on drift.

use schemars::{Schema, schema_for};

use crate::connector as c;
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
        // W8
        (
            "connector_register_request",
            schema_for!(c::RegisterRequest),
        ),
        (
            "connector_register_response",
            schema_for!(c::RegisterResponse),
        ),
        ("connector_manifest", schema_for!(c::ConnectorManifest)),
        ("connector_config_response", schema_for!(c::ConfigResponse)),
        ("connector_events_request", schema_for!(c::EventsRequest)),
        ("connector_events_response", schema_for!(c::EventsResponse)),
        ("connector_claim_request", schema_for!(c::ClaimRequest)),
        ("connector_claim_response", schema_for!(c::ClaimResponse)),
        ("connector_results_request", schema_for!(c::ResultsRequest)),
        ("connector_ok_response", schema_for!(c::OkResponse)),
        (
            "connector_credentials_request",
            schema_for!(c::CredentialsRequest),
        ),
        (
            "connector_credentials_response",
            schema_for!(c::CredentialsResponse),
        ),
        (
            "connector_link_start_request",
            schema_for!(c::LinkStartRequest),
        ),
        (
            "connector_link_start_response",
            schema_for!(c::LinkStartResponse),
        ),
        (
            "connector_file_put_response",
            schema_for!(c::FilePutResponse),
        ),
        ("connector_error_body", schema_for!(c::ErrorBody)),
        (
            "connector_event_message_posted",
            schema_for!(c::MessagePosted),
        ),
        (
            "connector_event_message_changed",
            schema_for!(c::MessageChanged),
        ),
        (
            "connector_event_bot_membership",
            schema_for!(c::BotMembership),
        ),
        (
            "connector_event_action_invoked",
            schema_for!(c::ActionInvoked),
        ),
        (
            "connector_event_command_invoked",
            schema_for!(c::CommandInvoked),
        ),
        (
            "connector_event_presence_removed",
            schema_for!(c::PresenceRemoved),
        ),
        (
            "connector_event_reaction_added",
            schema_for!(c::ReactionAdded),
        ),
        ("connector_inbound_event", schema_for!(c::InboundEvent)),
        ("connector_operation", schema_for!(c::Operation)),
        ("connector_op_message_post", schema_for!(c::MessagePostOp)),
        (
            "connector_op_message_update",
            schema_for!(c::MessageUpdateOp),
        ),
        (
            "connector_op_message_delete",
            schema_for!(c::MessageDeleteOp),
        ),
        ("connector_op_reaction", schema_for!(c::ReactionOp)),
        ("connector_op_status", schema_for!(c::StatusOp)),
        ("connector_op_file_fetch", schema_for!(c::FileFetchOp)),
        ("connector_op_user_lookup", schema_for!(c::UserLookupOp)),
        ("connector_op_spaces_list", schema_for!(c::SpacesListOp)),
        ("connector_op_space_members", schema_for!(c::SpaceMembersOp)),
        ("connector_op_history_read", schema_for!(c::HistoryReadOp)),
        (
            "connector_op_presence_provision",
            schema_for!(c::PresenceProvisionOp),
        ),
        (
            "connector_op_presence_change",
            schema_for!(c::PresenceChangeOp),
        ),
        (
            "connector_op_credentials_validate",
            schema_for!(c::CredentialsValidateOp),
        ),
        (
            "connector_result_message_post",
            schema_for!(c::MessagePostResult),
        ),
        (
            "connector_result_file_fetch",
            schema_for!(c::FileFetchResult),
        ),
        ("connector_result_users", schema_for!(c::UsersResult)),
        ("connector_result_spaces", schema_for!(c::SpacesResult)),
        ("connector_result_history", schema_for!(c::HistoryResult)),
        (
            "connector_result_presence_provision",
            schema_for!(c::PresenceProvisionResult),
        ),
        (
            "connector_result_credentials_validate",
            schema_for!(c::CredentialsValidateResult),
        ),
        // W7
        ("tools_tool_call", schema_for!(t::ToolCall)),
        ("tools_tool_result", schema_for!(t::ToolResult)),
        ("tools_tool_descriptor", schema_for!(t::ToolDescriptor)),
    ]
}
