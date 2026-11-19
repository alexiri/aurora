//! The effective send limit (C-912): the lowest set value of the instance, the org and the
//! connection, per window. One function for the API (`effective_rate` on `GET …/webhook`, the
//! ceiling check) and the gateway (the take at every send), so a lowered ceiling applies at once.
use serde::Serialize;

use crate::model::Rate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RateSource {
    Instance,
    Org,
    Connection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EffectiveRate {
    pub per_minute: u32,
    pub per_day: u32,
    /// The most specific level that lowers anything: `connection` if the connection's own limit is
    /// below the org's or the instance's, else `org` if the org's is below the instance's, else
    /// `instance`.
    pub source: RateSource,
}

/// `instance` is the pair of instance ceilings; `org` and `conn` hold a value only where set.
#[must_use]
pub fn effective_rate(
    instance: (u32, u32),
    org: (Option<u32>, Option<u32>),
    conn: &Rate,
) -> EffectiveRate {
    let org_min = org.0.map_or(instance.0, |o| o.min(instance.0));
    let org_day = org.1.map_or(instance.1, |o| o.min(instance.1));
    let per_minute = conn.per_minute.map_or(org_min, |c| c.min(org_min));
    let per_day = conn.per_day.map_or(org_day, |c| c.min(org_day));
    let source = if per_minute < org_min || per_day < org_day {
        RateSource::Connection
    } else if org_min < instance.0 || org_day < instance.1 {
        RateSource::Org
    } else {
        RateSource::Instance
    };
    EffectiveRate {
        per_minute,
        per_day,
        source,
    }
}

/// The ceiling a connection's own limits are checked against (`rate_above_ceiling`): the instance's
/// lowered by the org's.
#[must_use]
pub fn ceiling(instance: (u32, u32), org: (Option<u32>, Option<u32>)) -> (u32, u32) {
    (
        org.0.map_or(instance.0, |o| o.min(instance.0)),
        org.1.map_or(instance.1, |o| o.min(instance.1)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(m: Option<u32>, d: Option<u32>) -> Rate {
        Rate {
            per_minute: m,
            per_day: d,
        }
    }

    #[test]
    fn the_lowest_set_value_wins_and_the_source_names_who_lowered_it() {
        let inst = (30, 500);
        let none = (None, None);
        let e = effective_rate(inst, none, &rate(None, None));
        assert_eq!(
            (e.per_minute, e.per_day, e.source),
            (30, 500, RateSource::Instance)
        );
        let e = effective_rate(inst, (Some(10), None), &rate(None, None));
        assert_eq!(
            (e.per_minute, e.per_day, e.source),
            (10, 500, RateSource::Org)
        );
        let e = effective_rate(inst, (Some(10), Some(100)), &rate(Some(5), None));
        assert_eq!(
            (e.per_minute, e.per_day, e.source),
            (5, 100, RateSource::Connection)
        );
        // An org value above the instance's changes nothing; neither does a connection's above the org's.
        let e = effective_rate(inst, (Some(99), Some(9999)), &rate(Some(60), Some(2000)));
        assert_eq!(
            (e.per_minute, e.per_day, e.source),
            (30, 500, RateSource::Instance)
        );
        let e = effective_rate(inst, (Some(10), None), &rate(Some(20), None));
        assert_eq!((e.per_minute, e.source), (10, RateSource::Org));
        // A lowered org ceiling applies to an existing connection that set more.
        assert_eq!(ceiling(inst, (Some(7), None)), (7, 500));
    }
}
