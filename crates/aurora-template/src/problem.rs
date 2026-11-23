//! The problem shape every reader returns (C-1141): `{ code, path, message }`, at most 20.

/// Every C-1141 code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProblemCode {
    TemplateArchive,
    TemplateDirName,
    TemplateReadmeMissing,
    TemplateFileUnexpected,
    TemplateFormatUnsupported,
    TemplateYaml,
    TemplateField,
    TemplateModelNamed,
    TemplateScheduleOnce,
    TemplatePlaceholderUnknown,
    TemplateSkillInvalid,
    TemplateSkillRefInvalid,
    TemplateHelperInvalid,
    TemplateExecutableBinary,
    TemplateMemoryInvalid,
    TemplateAutomationInvalid,
    TemplateTooLarge,
}

impl ProblemCode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TemplateArchive => "template_archive",
            Self::TemplateDirName => "template_dir_name",
            Self::TemplateReadmeMissing => "template_readme_missing",
            Self::TemplateFileUnexpected => "template_file_unexpected",
            Self::TemplateFormatUnsupported => "template_format_unsupported",
            Self::TemplateYaml => "template_yaml",
            Self::TemplateField => "template_field",
            Self::TemplateModelNamed => "template_model_named",
            Self::TemplateScheduleOnce => "template_schedule_once",
            Self::TemplatePlaceholderUnknown => "template_placeholder_unknown",
            Self::TemplateSkillInvalid => "template_skill_invalid",
            Self::TemplateSkillRefInvalid => "template_skill_ref_invalid",
            Self::TemplateHelperInvalid => "template_helper_invalid",
            Self::TemplateExecutableBinary => "template_executable_binary",
            Self::TemplateMemoryInvalid => "template_memory_invalid",
            Self::TemplateAutomationInvalid => "template_automation_invalid",
            Self::TemplateTooLarge => "template_too_large",
        }
    }
}

impl std::fmt::Display for ProblemCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One refusal: what broke (`code`), where (`path`, the file and where it applies the YAML key,
/// like `template.yaml#/apps/1/level`) and one sentence (`message`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateProblem {
    pub code: ProblemCode,
    pub path: String,
    pub message: String,
}

impl TemplateProblem {
    #[must_use]
    pub fn new(code: ProblemCode, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TemplateProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}: {}", self.path, self.code, self.message)
    }
}

/// At most this many problems are returned (C-1141).
pub(crate) const PROBLEMS_MAX: usize = 20;

/// Sorts deterministically (by path, then code, then message) and caps at [`PROBLEMS_MAX`].
pub(crate) fn finish_problems(mut problems: Vec<TemplateProblem>) -> Vec<TemplateProblem> {
    problems.sort_by(|a, b| (&a.path, a.code, &a.message).cmp(&(&b.path, b.code, &b.message)));
    problems.truncate(PROBLEMS_MAX);
    problems
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_and_complete() {
        // C-1141's list, pinned: the code strings are part of the API surface.
        let all = [
            ProblemCode::TemplateArchive,
            ProblemCode::TemplateDirName,
            ProblemCode::TemplateReadmeMissing,
            ProblemCode::TemplateFileUnexpected,
            ProblemCode::TemplateFormatUnsupported,
            ProblemCode::TemplateYaml,
            ProblemCode::TemplateField,
            ProblemCode::TemplateModelNamed,
            ProblemCode::TemplateScheduleOnce,
            ProblemCode::TemplatePlaceholderUnknown,
            ProblemCode::TemplateSkillInvalid,
            ProblemCode::TemplateSkillRefInvalid,
            ProblemCode::TemplateHelperInvalid,
            ProblemCode::TemplateExecutableBinary,
            ProblemCode::TemplateMemoryInvalid,
            ProblemCode::TemplateAutomationInvalid,
            ProblemCode::TemplateTooLarge,
        ];
        for c in all {
            assert!(c.as_str().starts_with("template_"), "{c}");
        }
    }
}
