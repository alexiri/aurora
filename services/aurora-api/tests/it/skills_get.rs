//! FIX-291: `skill_get` stays within the tool-result limit (64 KiB). A text that does not fit is
//! cut by the tool itself, which says so and names the whole file in the agent's home.

use super::*;

const TOOL_RESULT_MAX: usize = 64 * 1024;

#[tokio::test]
async fn fix291_skill_get_says_when_it_cut_a_text_and_where_the_whole_file_is() {
    let r = rig().await;
    let b = r.booted("reader").await;
    // Instructions of three-byte characters and a file of lines: neither fits one result.
    let (s, v) = r
        .user(
            Method::POST,
            &format!("/v1/agents/{}/skills", b.agent),
            Some(json!({"name": "big", "description": "d", "body": "漢".repeat(40_000)})),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    let lines = "line of text\n".repeat(10_000);
    let (s, v) = r
        .raw_put(
            &r.cookie,
            &format!("/v1/skills/{id}/files/ref/data.txt"),
            lines.as_bytes(),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    for (args, field, whole, path) in [
        (
            json!({"name": "big", "path": "ref/data.txt"}),
            "content",
            lines.clone(),
            "$SKILLS_DIR/big/ref/data.txt",
        ),
        (
            json!({"name": "big"}),
            "body",
            "漢".repeat(40_000),
            "$SKILLS_DIR/big/SKILL.md",
        ),
    ] {
        let v = r.tool(&b, "skill_get", args).await;
        assert_eq!(v["ok"], true, "{v}");
        let size = v["content"].to_string().len();
        assert!(size <= TOOL_RESULT_MAX, "{size}");
        assert!(size > TOOL_RESULT_MAX - 1_024, "most of it is used: {size}");
        let out = &v["content"][0]["value"];
        assert_eq!(out["truncated"], true, "{field}");
        let text = out[field].as_str().unwrap();
        assert!(
            whole.starts_with(text) && text.len() < whole.len(),
            "a prefix"
        );
        let note = out["note"].as_str().unwrap();
        assert!(note.contains(path) && note.contains("65536"), "{note}");
    }

    // What fits is returned whole, and says nothing.
    let (s, v) = r
        .raw_put(&r.cookie, &format!("/v1/skills/{id}/files/s.txt"), b"small")
        .await;
    assert_eq!(s, 200, "{v}");
    let v = r
        .tool(&b, "skill_get", json!({"name": "big", "path": "s.txt"}))
        .await;
    let out = &v["content"][0]["value"];
    assert_eq!(out["content"], "small", "{v}");
    assert!(
        out.get("truncated").is_none() && out.get("note").is_none(),
        "{v}"
    );
}
