//! (P5-SKILLS-1, C-1175) Which tool call is the harness loading a skill: the folder name that
//! goes into `tool.started`'s `skill` field. Two ways are visible in a stream: the harness's own
//! skill tool (its argument names the skill) and a read of `<skills folder>/<name>/SKILL.md` by a
//! file-read tool or a plain read command in a shell. Only the folder name leaves this module,
//! never a path or a command; the API counts it only for a skill that reaches the agent.

use serde_json::Value;

use crate::boot::HOME_DIR;

/// Shell commands that only read a file.
const READERS: [&str; 7] = ["cat", "head", "tail", "sed", "less", "bat", "nl"];

/// A skill's folder name (the stored skills' own grammar, checked again by the API).
fn folder_name(name: &str) -> Option<String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    ok.then(|| name.to_owned())
}

/// The skill whose instructions a read of `path` loads: `<name>/SKILL.md` directly under the
/// harness's skills folder (`skills_dir`, relative to the home folder), written absolute, from
/// the home folder (`~`, `$HOME`), through `$SKILLS_DIR`, or relative to the home folder the
/// harness runs in.
pub fn of_path(path: &str, skills_dir: &str) -> Option<String> {
    let under_skills = ["$SKILLS_DIR/", "${SKILLS_DIR}/"]
        .iter()
        .find_map(|p| path.strip_prefix(p));
    let rest = under_skills.or_else(|| {
        let home_relative = [
            format!("{HOME_DIR}/"),
            "~/".to_owned(),
            "$HOME/".to_owned(),
            "${HOME}/".to_owned(),
            "./".to_owned(),
        ]
        .iter()
        .find_map(|p| path.strip_prefix(p.as_str()))
        .unwrap_or(path);
        home_relative
            .strip_prefix(skills_dir)
            .and_then(|r| r.strip_prefix('/'))
    })?;
    let (name, file) = rest.split_once('/')?;
    if file != "SKILL.md" {
        return None;
    }
    folder_name(name)
}

/// The skill a shell command loads: one of its simple commands only reads files, and one of
/// them is a skill's `SKILL.md`. A command that writes (a redirect, `sed -i`) is no read.
pub fn of_command(command: &str, skills_dir: &str) -> Option<String> {
    command.split(['\n', ';', '|', '&']).find_map(|part| {
        let mut words = part
            .split_whitespace()
            .map(|w| w.trim_matches(|c| c == '"' || c == '\''));
        let program = words.next()?;
        let program = program.rsplit('/').next().unwrap_or(program);
        if !READERS.contains(&program) {
            return None;
        }
        let args: Vec<&str> = words.collect();
        let writes = args.iter().any(|a| a.starts_with('>'))
            || (program == "sed"
                && args
                    .iter()
                    .any(|a| a.starts_with("-i") || a.starts_with("--in-place")));
        if writes {
            return None;
        }
        args.iter().find_map(|a| of_path(a, skills_dir))
    })
}

/// The string argument of `args` under the first of `keys` that has one.
fn arg<'a>(args: Option<&'a Value>, keys: &[&str]) -> Option<&'a str> {
    let o = args?.as_object()?;
    keys.iter().find_map(|k| o.get(*k).and_then(Value::as_str))
}

/// A file-read tool's call: the path under one of the keys the harnesses use.
pub fn of_read(args: Option<&Value>, skills_dir: &str) -> Option<String> {
    of_path(arg(args, &["file_path", "filePath", "path"])?, skills_dir)
}

/// A shell tool's call.
pub fn of_shell(args: Option<&Value>, skills_dir: &str) -> Option<String> {
    of_command(arg(args, &["command", "cmd"])?, skills_dir)
}

/// The harness's own skill tool: its argument is the skill's name (under `keys`).
pub fn of_skill_tool(args: Option<&Value>, keys: &[&str]) -> Option<String> {
    folder_name(arg(args, keys)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DIR: &str = ".agents/skills";

    #[test]
    fn a_skill_md_under_the_skills_folder_names_its_skill() {
        for p in [
            "/workspace/.agents/skills/pdf/SKILL.md",
            "~/.agents/skills/pdf/SKILL.md",
            "$HOME/.agents/skills/pdf/SKILL.md",
            "${HOME}/.agents/skills/pdf/SKILL.md",
            "$SKILLS_DIR/pdf/SKILL.md",
            "${SKILLS_DIR}/pdf/SKILL.md",
            ".agents/skills/pdf/SKILL.md",
            "./.agents/skills/pdf/SKILL.md",
        ] {
            assert_eq!(of_path(p, DIR).as_deref(), Some("pdf"), "{p}");
        }
        assert_eq!(
            of_path(
                "/workspace/.codex/skills/aurora/a-b2/SKILL.md",
                ".codex/skills/aurora"
            )
            .as_deref(),
            Some("a-b2")
        );
    }

    #[test]
    fn other_files_and_other_folders_name_nothing() {
        for p in [
            "/workspace/.agents/skills/pdf/forms.md",
            "/workspace/.agents/skills/pdf/refs/SKILL.md",
            "/workspace/.agents/skills/SKILL.md",
            "/workspace/.agents/skills/../x/SKILL.md",
            "/workspace/.agents/skills/Pdf/SKILL.md",
            // A repository's own skills folder is not the agent's.
            "/workspace/repo/.agents/skills/pdf/SKILL.md",
            "/etc/.agents/skills/pdf/SKILL.md",
            "SKILL.md",
            "",
        ] {
            assert_eq!(of_path(p, DIR), None, "{p}");
        }
    }

    #[test]
    fn a_plain_read_command_counts_and_nothing_else() {
        let ok = [
            "cat .agents/skills/pdf/SKILL.md",
            "sed -n '1,200p' \"$SKILLS_DIR/pdf/SKILL.md\"",
            "cd /workspace && /bin/cat '~/.agents/skills/pdf/SKILL.md' | head -50",
            "ls; head -n 40 /workspace/.agents/skills/pdf/SKILL.md",
        ];
        for c in ok {
            assert_eq!(of_command(c, DIR).as_deref(), Some("pdf"), "{c}");
        }
        for c in [
            "ls .agents/skills/pdf/SKILL.md",
            "rm .agents/skills/pdf/SKILL.md",
            "sed -i s/a/b/ .agents/skills/pdf/SKILL.md",
            "cat notes.md > .agents/skills/pdf/SKILL.md",
            "echo .agents/skills/pdf/SKILL.md",
            "cat .agents/skills/pdf/forms.md",
        ] {
            assert_eq!(of_command(c, DIR), None, "{c}");
        }
    }

    #[test]
    fn tool_arguments_are_read_by_their_keys() {
        let path = "/workspace/.agents/skills/pdf/SKILL.md";
        for k in ["file_path", "filePath", "path"] {
            assert_eq!(
                of_read(Some(&json!({ k: path })), DIR).as_deref(),
                Some("pdf")
            );
        }
        assert_eq!(of_read(None, DIR), None);
        assert_eq!(
            of_shell(Some(&json!({"command": format!("cat {path}")})), DIR).as_deref(),
            Some("pdf")
        );
        assert_eq!(
            of_skill_tool(Some(&json!({"skill": "pdf", "args": "x"})), &["skill"]).as_deref(),
            Some("pdf")
        );
        // A name that is no folder name (a plugin's, a path) names nothing.
        for bad in ["plugin:pdf", "../pdf", "PDF", ""] {
            assert_eq!(
                of_skill_tool(Some(&json!({"skill": bad})), &["skill"]),
                None
            );
        }
    }
}
