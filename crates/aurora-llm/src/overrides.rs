use std::collections::BTreeMap;

/// (C-812, D-260(e)) `llm.dev_upstream_overrides`: a pinned provider or mint origin (`https://oauth2.googleapis.com`)
/// mapped to a loopback target. The config layer has already refused anything but loopback targets and refuses the
/// whole table outside tests and development. Every pinned origin of the providers and of the mint service goes through
/// [`Overrides::origin`], so one table serves them all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overrides(BTreeMap<String, String>);

impl Overrides {
    #[must_use]
    pub fn new(map: &BTreeMap<String, String>) -> Self {
        Self(
            map.iter()
                .map(|(k, v)| (k.clone(), v.trim_end_matches('/').to_owned()))
                .collect(),
        )
    }

    /// The origin to use for the pinned `origin`: its override target, else `origin` itself.
    #[must_use]
    pub fn origin(&self, origin: &str) -> String {
        self.0
            .get(origin)
            .cloned()
            .unwrap_or_else(|| origin.to_owned())
    }

    /// Whether `origin` is replaced.
    #[must_use]
    pub fn contains(&self, origin: &str) -> bool {
        self.0.contains_key(origin)
    }
}
