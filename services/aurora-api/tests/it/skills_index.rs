//! FIX-291: a skill's description may be 1,024 characters, and the skill index (the
//! descriptions every harness lists from the projected `SKILL.md` files, and `skill_list`) is
//! no bigger than it could be before: 500 characters a skill together, the long ones folded.

use super::*;

/// The `description:` of every projected `SKILL.md`, by skill name.
async fn index(r: &Rig, b: &Booted) -> std::collections::BTreeMap<String, String> {
    let claim = aurora_db::repo::hosted::HomeSyncClaim {
        sandbox_id: b.sandbox,
        boot_id: b.boot,
        agent_id: b.agent,
        generation: 1,
        harness_id: Some("fake".into()),
    };
    let mut conn = r.h.pool().acquire().await.unwrap();
    let parts = aurora_api::domains::hosted::home::render(&r.h.state, &mut conn, &claim)
        .await
        .unwrap();
    parts
        .iter()
        .filter(|p| p.part.kind.as_str() == "skills")
        .flat_map(|p| p.part.entries.iter())
        .filter_map(|e| {
            let name = e["path"]
                .as_str()?
                .strip_prefix(".claude/skills/")?
                .strip_suffix("/SKILL.md")?
                .to_owned();
            let line = e["content"]
                .as_str()?
                .lines()
                .find_map(|l| l.strip_prefix("description: "))?;
            Some((name, serde_json::from_str::<String>(line).unwrap()))
        })
        .collect()
}

async fn create(r: &Rig, agent: Uuid, name: &str, description: &str) -> (u16, Value) {
    r.user(
        Method::POST,
        &format!("/v1/agents/{agent}/skills"),
        Some(json!({"name": name, "description": description, "body": "b"})),
    )
    .await
}

#[tokio::test]
async fn fix291_a_description_holds_1024_characters() {
    let r = rig().await;
    let a = r.agent("a1").await;
    let (s, v) = create(&r, a, "long", &"é".repeat(1_024)).await;
    assert_eq!(s, 201, "{v}");
    let (s, v) = create(&r, a, "longer", &"d".repeat(1_025)).await;
    assert_eq!(s, 400, "{v}");
    assert!(v.to_string().contains("1024"), "{v}");
}

#[tokio::test]
async fn fix291_the_skill_index_keeps_its_size_and_folds_long_descriptions() {
    let r = rig().await;
    let b = r.booted("idx").await;
    let long = |c: char| c.to_string().repeat(1_024);
    // One long description beside short ones fits the index: nothing is folded.
    for (name, d) in [
        ("short-a", "Short a".to_owned()),
        ("short-b", "Short b".to_owned()),
        ("long-a", long('a')),
    ] {
        let (s, v) = create(&r, b.agent, name, &d).await;
        assert_eq!(s, 201, "{v}");
    }
    let i = index(&r, &b).await;
    assert_eq!(i["long-a"], long('a'), "there is room for it");

    // More long ones than the index holds: they share what the short ones leave, and no
    // description of up to 500 characters is touched.
    let medium = "m".repeat(500);
    for (name, d) in [
        ("long-b", long('b')),
        ("long-c", long('c')),
        ("long-d", long('d')),
        ("medium", medium.clone()),
    ] {
        let (s, v) = create(&r, b.agent, name, &d).await;
        assert_eq!(s, 201, "{v}");
    }
    let i = index(&r, &b).await;
    assert_eq!(i.len(), 7);
    let total: usize = i.values().map(|d| d.chars().count()).sum();
    assert!(total <= 7 * 500, "{total}");
    assert!(total > 7 * 500 - 10, "the room is used: {total}");
    assert_eq!(i["short-a"], "Short a");
    assert_eq!(i["short-b"], "Short b");
    assert_eq!(i["medium"], medium);
    let folded = &i["long-a"];
    assert!(
        folded.ends_with('…') && folded.starts_with("aaa"),
        "{folded}"
    );
    let n = folded.chars().count();
    assert!((500..1_024).contains(&n), "{n}");
    for name in ["long-b", "long-c", "long-d"] {
        assert_eq!(i[name].chars().count(), n, "an equal share");
    }

    // The agent's list is the same index; reading one skill gives its whole description.
    let v = r.tool(&b, "skill_list", json!({})).await;
    let listed = v["content"][0]["value"]["skills"].as_array().unwrap();
    for s in listed {
        let name = s["name"].as_str().unwrap();
        assert_eq!(s["description"], i[name], "{name}");
    }
    let v = r.tool(&b, "skill_get", json!({"name": "long-a"})).await;
    assert_eq!(v["content"][0]["value"]["description"], long('a'), "{v}");
}
