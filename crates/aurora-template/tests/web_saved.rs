//! P6-TPL-5: the zip the web dashboard's **Save as template** builds parses here with the same
//! reader the API and the CLI use. The fixture is generated and proved fresh by
//! `web/tests/template-fixture.test.ts` (`AURORA_TPL_FIXTURE=1 pnpm exec vitest run
//! tests/template-fixture.test.ts` in `web/` rebuilds it; without the variable that test compares
//! the committed bytes with a fresh build).

use aurora_template::{AppEntry, Level, SkillRef};

const FIXTURE: &[u8] = include_bytes!("fixtures/web-saved-template.zip");

#[test]
fn web_saved_template_parses() {
    let c = aurora_template::read_archive(FIXTURE).unwrap();

    // `template.yaml`: the meta and the two apps (one `act`, one custom with `tools:`; the level
    // is then not written and defaults here).
    assert_eq!(c.meta.id, "daneel");
    assert_eq!(c.meta.version, "1.0.0");
    assert_eq!(c.meta.name, "Daneel");
    assert_eq!(c.meta.summary, "Triage the team's pull requests.");
    assert_eq!(c.meta.apps.len(), 2);
    let AppEntry::App { app, required, level, tools, why } = &c.meta.apps[0] else {
        panic!("the first app entry is a single app");
    };
    assert_eq!(app, "github");
    assert!(required);
    assert_eq!(*level, Level::Act);
    assert!(tools.is_none());
    assert_eq!(why, "Reads and triages the pull requests.");
    let AppEntry::App { app, tools, .. } = &c.meta.apps[1] else {
        panic!("the second app entry is a single app");
    };
    assert_eq!(app, "linear");
    let tools = tools.as_ref().expect("the custom choice carries tools");
    assert_eq!(tools.allow, vec!["linear_issue_get"]);
    assert_eq!(tools.ask, vec!["linear_issue_create"]);

    // The library skill is a reference (C-1142), never a copy.
    assert_eq!(c.meta.skill_refs.len(), 1);
    let SkillRef::Library { address, ref_, path, skill, required, why } = &c.meta.skill_refs[0] else {
        panic!("the skill reference is a library");
    };
    assert_eq!(address.url, "https://gitlab.example.org/agents/skills.git");
    assert_eq!(ref_.as_deref(), Some("v1.2"));
    assert_eq!(path.as_deref(), Some("skills"));
    assert_eq!(skill, "incident-writeup");
    assert!(required);
    assert_eq!(why, "Writes the incident summary.");

    // The helper kept its bytes and its executable bit (the zip's external attributes).
    assert_eq!(c.helpers.len(), 1);
    assert_eq!(c.helpers[0].path, "tidy.sh");
    assert!(c.helpers[0].executable);
    assert_eq!(c.helpers[0].bytes, b"#!/bin/sh\necho tidied\n");

    // The rest of the agent came through the export part of the file.
    assert_eq!(c.agent_name, "Daneel");
    assert!(c.readme.contains("Made with Save as template on 2026-11-24"));
    assert_eq!(c.memories.len(), 1);
    assert!(c.memories[0].locked);
    assert_eq!(c.skills.len(), 1);
    assert_eq!(c.skills[0].name, "release-notes");
    assert_eq!(c.automations.len(), 1);
}
