//! FIX-291 (D-513): the skill size limits let real skill libraries in. The sizes of the
//! "real" fixtures are those measured in a public skill library; each limit still refuses one
//! over with its plain reason code, and the tier budgets are 4 / 8 / 16 MiB.

use aurora_core::limits;

use super::*;

fn item<'a>(v: &'a Value, path: &str) -> &'a Value {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["path"] == path)
        .unwrap_or_else(|| panic!("{path} missing in {v}"))
}

async fn browse(w: &W, lib: Uuid) -> Value {
    let (s, v) = w
        .call(
            &w.member,
            Method::GET,
            &format!(
                "/v1/skill-libraries/{lib}/skills?tier=agent&agent={}",
                w.agent
            ),
            None,
        )
        .await;
    assert_eq!(s, 200, "{v}");
    v
}

/// `SKILL.md` whose stored body is exactly `chars` characters (`md` appends one newline).
fn md_sized(chars: usize) -> Vec<u8> {
    md("d", &"b".repeat(chars - 1))
}

/// `total` characters of body and files, as `n` text files of at most 100 000 bytes each beside
/// a body of one `b` line (2 characters).
fn totalling(dir: &str, total: usize) -> Vec<Change> {
    let mut out = vec![put(&format!("{dir}/SKILL.md"), md("d", "b"))];
    let mut left = total - 2;
    let mut i = 0;
    while left > 0 {
        let n = left.min(100_000);
        out.push(put(&format!("{dir}/f{i}.txt"), vec![b'a'; n]));
        left -= n;
        i += 1;
    }
    out
}

#[tokio::test]
async fn fix291_skills_sized_like_the_largest_real_ones_are_listed_and_added() {
    let w = world().await;
    w.git.add_repo("acme/skills");
    // The largest body, total and text file: 32 625 characters of instructions, 224 187 in
    // all, 17 files, one of them 44 998 bytes.
    let (body, text, total) = (32_625, 44_998, 224_187);
    let mut changes = vec![
        put("creator/SKILL.md", md_sized(body)),
        put("creator/viewer.html", vec![b'v'; text]),
    ];
    let mut left = total - body - text;
    for i in 0..16 {
        let n = if i == 15 { left } else { 9_000 };
        left -= n;
        changes.push(put(&format!("creator/f{i}.txt"), vec![b'f'; n]));
    }
    // The largest single file that still reaches an agent's library: a 124 310-byte binary.
    changes.push(put("themes/SKILL.md", md("d", "b")));
    // (0xff: not UTF-8. A file of zero bytes is valid UTF-8 and would be sent as text.)
    changes.push(put("themes/showcase.pdf", vec![0xff; 124_310]));
    let sha = w.git.commit("acme/skills", "main", &changes);
    let lib = w.library().await;

    let v = browse(&w, lib).await;
    for dir in ["creator", "themes"] {
        let i = item(&v, dir);
        assert_eq!(i["valid"], true, "{i}");
        assert_eq!(i["reason_code"], Value::Null, "{i}");
        assert_eq!(i["blocked"], Value::Null, "{i}");
        let (s, a) = w.add_agent(lib, dir, &sha).await;
        assert_eq!(s, 201, "{dir}: {a}");
        assert_eq!(a["total_size"], i["size"], "{a}");
    }
    assert_eq!(item(&v, "creator")["size"], total, "{v}");
}

#[tokio::test]
async fn fix291_one_over_each_limit_is_refused_with_its_reason_code() {
    let w = world().await;
    w.git.add_repo("acme/skills");
    let mut changes = vec![
        put("body-at/SKILL.md", md_sized(limits::SKILL_BODY_MAX)),
        put("body-over/SKILL.md", md_sized(limits::SKILL_BODY_MAX + 1)),
        put("file-at/SKILL.md", md("d", "b")),
        put("file-at/x.bin", vec![0xff; limits::LIBRARY_FILE_MAX]),
        put("file-over/SKILL.md", md("d", "b")),
        put("file-over/x.bin", vec![0xff; limits::LIBRARY_FILE_MAX + 1]),
        // Within the byte limit of a file, but every line break takes two bytes as JSON.
        put("json-over/SKILL.md", md("d", "b")),
        put("json-over/lines.txt", vec![b'\n'; 100_000]),
    ];
    changes.extend(totalling("total-at", limits::SKILL_TOTAL_MAX));
    changes.extend(totalling("total-over", limits::SKILL_TOTAL_MAX + 1));
    let sha = w.git.commit("acme/skills", "main", &changes);
    let lib = w.library().await;
    let v = browse(&w, lib).await;

    assert_eq!(limits::SKILL_BODY_MAX, 48_000);
    assert_eq!(limits::SKILL_TOTAL_MAX, 300_000);
    assert_eq!(limits::LIBRARY_FILE_MAX, 160 * 1024);
    for dir in ["body-at", "file-at", "total-at"] {
        assert_eq!(item(&v, dir)["valid"], true, "{}", item(&v, dir));
        let (s, a) = w.add_agent(lib, dir, &sha).await;
        assert_eq!(s, 201, "{dir}: {a}");
    }
    for (dir, want, words) in [
        ("body-over", "body_too_large", "48000"),
        ("total-over", "too_large", "300000"),
        ("file-over", "file_too_large", "163840"),
        ("json-over", "file_too_large", "as JSON"),
    ] {
        let i = item(&v, dir);
        assert_eq!(i["valid"], false, "{i}");
        assert_eq!(i["reason_code"], want, "{dir}: {i}");
        assert!(i["reason"].as_str().unwrap().contains(words), "{i}");
        let (s, e) = w.add_agent(lib, dir, &sha).await;
        assert_eq!((s, code(&e)), (422, "library_skill_invalid"), "{e}");
    }
}

/// The open skill format's own maximum for a description is 1,024 characters.
#[tokio::test]
async fn fix291_a_library_skill_with_a_1024_character_description_is_addable() {
    let w = world().await;
    w.git.add_repo("acme/skills");
    let sha = w.git.commit(
        "acme/skills",
        "main",
        &[
            put("guide/SKILL.md", md(&"d".repeat(1_024), "b")),
            put("wordy/SKILL.md", md(&"d".repeat(1_025), "b")),
        ],
    );
    let lib = w.library().await;
    let v = browse(&w, lib).await;
    assert_eq!(item(&v, "guide")["valid"], true, "{v}");
    assert_eq!(item(&v, "wordy")["valid"], false, "{v}");
    let (s, a) = w.add_agent(lib, "guide", &sha).await;
    assert_eq!(s, 201, "{a}");
    assert_eq!(a["description"], "d".repeat(1_024), "stored whole");
}

#[tokio::test]
async fn fix291_tier_budgets_are_4_8_and_16_mib() {
    assert_eq!(limits::LIBRARY_BYTES_PER_AGENT, 4 * 1024 * 1024);
    assert_eq!(limits::LIBRARY_BYTES_PER_WORKSPACE, 8 * 1024 * 1024);
    assert_eq!(limits::LIBRARY_BYTES_PER_ORG, 16 * 1024 * 1024);

    let w = world().await;
    w.git.add_repo("acme/skills");
    // 13 skills of the largest size fit in an agent's 4 MiB; the 14th does not.
    let changes: Vec<Change> = (0..14)
        .flat_map(|i| totalling(&format!("s{i:02}"), limits::SKILL_TOTAL_MAX))
        .collect();
    let sha = w.git.commit("acme/skills", "main", &changes);
    let lib = w.library().await;
    for i in 0..13 {
        let (s, v) = w.add_agent(lib, &format!("s{i:02}"), &sha).await;
        assert_eq!(s, 201, "{i}: {v}");
    }
    let (s, v) = w.add_agent(lib, "s13", &sha).await;
    assert_eq!((s, code(&v)), (422, "library_budget_exceeded"), "{v}");
    assert_eq!(v["error"]["details"]["tier"], "agent", "{v}");
    assert_eq!(v["error"]["details"]["bytes_limit"], 4_194_304, "{v}");
}
