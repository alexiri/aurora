//! The unit-test fixtures of this crate that name real products (Jira, Atlassian, Microsoft 365,
//! CalDAV, GitHub, GitLab) the way C-1141's own examples do. This is the one file the SEC-33
//! allow-list names (`scripts/guards/integration-names.allow`): the crate parses template data,
//! and these names appear only in test fixtures, never in shipped behaviour. Everything here is
//! compiled for tests only (`#[cfg(test)]` on the `mod` declaration in `lib.rs`).

use crate::model::{
    AppEntry, Automation, Effort, InputDecl, InputKind, Level, MemoryFile, SkillRef,
    TemplateContent, TemplateFile, TemplateMeta, TemplateSkill, TreeFile, Verification,
};

// ---- the service-desk inputs and apps (meta.rs) -------------------------------------------------

/// The desk input id the fixtures use (a Jira-shaped project key).
pub(crate) const DESK_INPUT: &str = "jira_project";

/// (input id, accepted) for the input-id grammar test: the fixture and its near-misses.
pub(crate) const DESK_INPUT_IDS: &[(&str, bool)] = &[
    ("jira_project", true),
    ("Jira_project", false),
    ("xjira_project_xxxxxxxxxxxxxxxxxxxxxx", false),
];

/// The `inputs:`/`apps:` fixture of the `inputs_and_apps` test: C-1141's example shapes (a Jira
/// input, an Atlassian app with tools, a Microsoft 365 / CalDAV `any_of` group).
pub(crate) const INPUTS_AND_APPS_YAML: &str = "format: 1\nid: t\nversion: 1.0.0\nname: N\nsummary: S\ninputs:\n  - id: jira_project\n    label: Jira project key\n    pattern: \"^[A-Z][A-Z0-9]+$\"\n    default: OPS\napps:\n  - app: atlassian\n    level: ask\n    tools:\n      allow: [jira_search]\n      ask: [jira_issue_create]\n    why: Tickets.\n  - any_of: [microsoft-365, caldav]\n    level: ask\n    why: Calendar.\nskills:\n  - organization: deploy-checklist\n    required: false\n    why: Expected.\n";

/// The `apps:` fixtures of the `app_rules` test: duplicate entries, `app` with `any_of`, empty
/// `tools:`, and overlapping `tools:` lists.
pub(crate) const APP_DUP_YAML: &str = "format: 1\nid: t\nversion: 1.0.0\nname: N\nsummary: S\napps:\n  - app: atlassian\n    why: a\n  - app: atlassian\n    why: b\n";
pub(crate) const APP_BOTH_YAML: &str = "format: 1\nid: t\nversion: 1.0.0\nname: N\nsummary: S\napps:\n  - app: atlassian\n    any_of: [caldav, github]\n    why: a\n";
pub(crate) const APP_EMPTY_TOOLS_YAML: &str = "format: 1\nid: t\nversion: 1.0.0\nname: N\nsummary: S\napps:\n  - app: atlassian\n    tools: {}\n    why: a\n";
pub(crate) const APP_OVERLAP_YAML: &str = "format: 1\nid: t\nversion: 1.0.0\nname: N\nsummary: S\napps:\n  - app: atlassian\n    tools:\n      allow: [a]\n      ask: [a]\n    why: x\n";

/// The library-address fixtures of the `skill_refs` test: a GitLab page URL as people paste it
/// (normalized to the clone address), and the ssh form that is refused.
pub(crate) const LIBRARY_PAGE_YAML: &str = "format: 1\nid: t\nversion: 1.0.0\nname: N\nsummary: S\nskills:\n  - library: https://gitlab.cern.ch/agents/skills/-/tree/main/team\n    skill: incident-writeup\n    why: Writes the summary.\n";
pub(crate) const LIBRARY_NORMALIZED: &str = "https://gitlab.cern.ch/agents/skills";
pub(crate) const LIBRARY_SSH_YAML: &str = "format: 1\nid: t\nversion: 1.0.0\nname: N\nsummary: S\nskills:\n  - library: git@github.com:a/b.git\n    skill: x\n    why: y\n";

// ---- the manifests (manifest.rs) ------------------------------------------------------------------

/// The GitHub-releases manifest of the `ok_manifest` and `rendered_url_checks` tests.
pub(crate) const MANIFEST_OK_YAML: &str = "api: 1\nrepo:\n  name: CERN agent templates\n  description: Templates maintained by the CERN agent team.\n  homepage: https://example.org/agents\n  contact: mailto:agents@example.org\npackages:\n  url: https://github.com/cern/agent-templates/releases/download/{id}-v{version}/{id}-{version}.zip\n";
pub(crate) const MANIFEST_OK_URL: &str =
    "https://github.com/cern/agent-templates/releases/download/{id}-v{version}/{id}-{version}.zip";
pub(crate) const MANIFEST_OK_RENDERED: &str = "https://github.com/cern/agent-templates/releases/download/service-monitor-v1.2.3/service-monitor-1.2.3.zip";
/// The generic-packages manifest of the `generic_packages_shape` test.
pub(crate) const MANIFEST_GENERIC_YAML: &str = "api: 1\nrepo:\n  name: n\n  description: d\npackages:\n  url: https://gitlab.example.org/api/v4/projects/42/packages/generic/{id}/{version}/{id}-{version}.zip\n";
pub(crate) const MANIFEST_GENERIC_RENDERED: &str =
    "https://gitlab.example.org/api/v4/projects/42/packages/generic/a/0.1.0/a-0.1.0.zip";

// ---- the skill-reference keys (hash.rs) ------------------------------------------------------------

/// The library addresses and the keys they render to in the `ref_keys` test: a library with a
/// branch, a folder and a skill, and the bare clone.
pub(crate) const LIBRARY_CLONE: &str = "https://gitlab.cern.ch/agents/skills.git";
pub(crate) const LIBRARY_CLONE_KEY: &str =
    "library|https://gitlab.cern.ch/agents/skills.git|main|team|incident-writeup";
pub(crate) const LIBRARY_BARE_CLONE: &str = "https://github.com/a/b.git";
pub(crate) const LIBRARY_BARE_CLONE_KEY: &str = "library|https://github.com/a/b.git|||x";

// ---- substitution texts (render.rs) -----------------------------------------------------------------

/// The text of the one-pass substitution test: the inserted value itself carries tokens.
pub(crate) const ONE_PASS_TEXT: &str =
    "Hello {person.name}, {input.jira_project} and {input.key} stay {agent.name}.";
/// The text whose only input placeholder is the fixture input.
pub(crate) const PLACEHOLDER_TEXT: &str = "a {input.x} b {agent.name} {input.jira_project}";

// ---- the service-monitor template tree (validate.rs, archive.rs, render.rs) -----------------------

/// The `template.yaml` of the service-monitor fixture (the Jira input its texts render).
pub(crate) const TEMPLATE_YAML: &str = "format: 1\nid: service-monitor\nversion: 1.0.0\nname: Service monitor\nsummary: Watches dashboards and tickets.\ninputs:\n  - id: jira_project\n    label: Jira project key\n    pattern: \"^[A-Z][A-Z0-9]+$\"\n";

pub(crate) const AGENT_YAML: &str =
    "format: 1\nname: Monitor\npurpose: monitor\nmodel: default\nmay_message_first: false\n";

pub(crate) fn file(path: &str, text: &str) -> TreeFile {
    TreeFile {
        path: path.to_owned(),
        bytes: text.as_bytes().to_vec(),
        mode: 0o100644,
    }
}

pub(crate) fn exec(path: &str, text: &str) -> TreeFile {
    TreeFile {
        path: path.to_owned(),
        bytes: text.as_bytes().to_vec(),
        mode: 0o100755,
    }
}

/// A full valid template used by the validate, archive, render and hash tests.
pub(crate) fn sample_files() -> Vec<TreeFile> {
    vec![
        file("service-monitor/template.yaml", TEMPLATE_YAML),
        file("service-monitor/agent.yaml", AGENT_YAML),
        file(
            "service-monitor/instructions.md",
            "Watch {input.jira_project} tickets and the morning board.\n",
        ),
        file(
            "service-monitor/README.md",
            "# Service monitor\nFor the service desk.\n",
        ),
        file(
            "service-monitor/memory/service-notes.md",
            "---\nkey: service-notes\nupdated: 2026-01-01\nlocked: true\n---\n{input.jira_project} is the desk's project.\n",
        ),
        file(
            "service-monitor/skills/what-is-on/SKILL.md",
            "---\nname: what-is-on\nuse-it-when: The person asks about {input.jira_project}.\n---\nLook at {input.jira_project}.\n",
        ),
        exec(
            "service-monitor/skills/what-is-on/run.sh",
            "#!/bin/sh\necho run\n",
        ),
        file(
            "service-monitor/helpers/alert-summary.sh",
            "#!/bin/sh\necho summary\n",
        ),
        file(
            "service-monitor/automations/morning-check.yaml",
            "kind: schedule\nname: Morning check\nprompt: Check the {input.jira_project} dashboards.\ncadence:\n  cron: \"0 8 * * 1-5\"\nenabled: true\n",
        ),
        file(
            "service-monitor/automations/escalate.yaml",
            "kind: webhook\nname: Escalate\ninstructions: Page the on-call for {input.jira_project}.\nverification: signed\n",
        ),
    ]
}

/// The parsed content of [`sample_files`]; the render test reads its fields.
pub(crate) fn sample_content() -> TemplateContent {
    crate::read_tree("service-monitor", &sample_files()).unwrap()
}

// ---- the hash tests' own template (hash.rs) ---------------------------------------------------------

/// The metadata of the hash tests' template (the same service-desk fixture, built as data).
pub(crate) fn hash_meta() -> TemplateMeta {
    TemplateMeta {
        format: 1,
        id: "test".into(),
        version: "1.0.0".into(),
        name: "Test".into(),
        summary: "A test.".into(),
        min_core: None,
        effort: Some(Effort::Medium),
        inputs: vec![InputDecl {
            id: "jira_project".into(),
            label: "Jira project key".into(),
            help: None,
            kind: InputKind::Text,
            required: true,
            pattern: Some("^[A-Z]+$".into()),
            max_length: Some(20),
            options: vec![],
            default: Some("OPS".into()),
        }],
        apps: vec![AppEntry::App {
            app: "atlassian".into(),
            required: true,
            level: Level::Ask,
            tools: None,
            why: "Tickets.".into(),
        }],
        skill_refs: vec![SkillRef::Organization {
            name: "deploy-checklist".into(),
            required: false,
            why: "Expected.".into(),
        }],
    }
}

/// The full content of the hash tests' template (golden hashes are pinned to it).
pub(crate) fn hash_content() -> TemplateContent {
    TemplateContent {
        meta: hash_meta(),
        agent_name: "Monitor".into(),
        may_message_first: false,
        instructions: "Do the thing.".into(),
        readme: "# Test\nWhat it does.\n".into(),
        memories: vec![MemoryFile {
            key: "notes".into(),
            content: "Care about weekends.".into(),
            locked: true,
        }],
        skills: vec![TemplateSkill {
            name: "what-is-on".into(),
            use_it_when: "The person asks what is on.".into(),
            body: "Look at the calendar.".into(),
            files: vec![TemplateFile {
                path: "format.py".into(),
                bytes: b"print(1)\n".to_vec(),
                executable: true,
            }],
        }],
        helpers: vec![TemplateFile {
            path: "alert-summary.sh".into(),
            bytes: b"#!/bin/sh\necho hi\n".to_vec(),
            executable: true,
        }],
        automations: vec![
            Automation::Schedule {
                name: "Morning check".into(),
                prompt: "Check the dashboards for {input.jira_project}.".into(),
                cron: "0 8 * * 1-5".into(),
            },
            Automation::Webhook {
                name: "Escalate".into(),
                instructions: "Page the on-call.".into(),
                verification: Verification::Signed,
            },
        ],
        avatar: Some(("avatar.png".into(), vec![0x89, b'P', b'N', b'G'])),
    }
}
