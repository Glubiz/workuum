use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MinFree {
    Bytes(u64),
    Percent(f64),
}

impl MinFree {
    /// Bytes still missing to reach the target, 0 when already met.
    pub fn deficit(&self, total: u64, available: u64) -> u64 {
        let target = match self {
            MinFree::Bytes(bytes) => *bytes,
            MinFree::Percent(percent) => (total as f64 * percent / 100.0) as u64,
        };
        target.saturating_sub(available)
    }
}

/// Takes candidates oldest first, measuring only those taken, until `deficit` bytes are covered.
pub fn plan_evictions<T>(
    mut candidates: Vec<(T, SystemTime)>,
    deficit: u64,
    mut size_of: impl FnMut(&T) -> u64,
) -> Vec<(T, u64)> {
    candidates.sort_by_key(|(_, last_activity)| *last_activity);
    let mut chosen = Vec::new();
    let mut total = 0u64;
    for (candidate, _) in candidates {
        if total >= deficit {
            break;
        }
        let size = size_of(&candidate);
        total = total.saturating_add(size);
        chosen.push((candidate, size));
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn deficit_table() {
        let cases = [
            (MinFree::Bytes(100), 1000, 30, 70),
            (MinFree::Bytes(100), 1000, 100, 0),
            (MinFree::Bytes(100), 1000, 500, 0),
            (MinFree::Percent(10.0), 1000, 30, 70),
            (MinFree::Percent(12.5), 1000, 0, 125),
            (MinFree::Percent(10.0), 1000, 100, 0),
            (MinFree::Percent(10.0), 1000, 900, 0),
            (MinFree::Percent(100.0), 1000, 0, 1000),
        ];
        for (min_free, total, available, expected) in cases {
            assert_eq!(min_free.deficit(total, available), expected, "{min_free:?}");
        }
    }

    #[test]
    fn zero_deficit_measures_nothing() {
        let mut calls = 0;
        let plan = plan_evictions(vec![("a", at(1))], 0, |_| {
            calls += 1;
            10
        });
        assert!(plan.is_empty());
        assert_eq!(calls, 0);
    }

    #[test]
    fn exactly_covered() {
        let plan = plan_evictions(vec![("a", at(1)), ("b", at(2))], 10, |_| 5);
        assert_eq!(plan, vec![("a", 5), ("b", 5)]);
    }

    #[test]
    fn not_coverable_takes_everything() {
        let plan = plan_evictions(vec![("a", at(1)), ("b", at(2))], 100, |_| 5);
        assert_eq!(plan.len(), 2);
    }

    #[test]
    fn oldest_first() {
        let plan = plan_evictions(
            vec![("new", at(30)), ("old", at(10)), ("mid", at(20))],
            3,
            |_| 1,
        );
        let order: Vec<_> = plan.iter().map(|(name, _)| *name).collect();
        assert_eq!(order, ["old", "mid", "new"]);
    }

    #[test]
    fn stops_measuring_once_covered() {
        let mut measured = Vec::new();
        let plan = plan_evictions(vec![("c", at(3)), ("a", at(1)), ("b", at(2))], 8, |name| {
            measured.push(*name);
            5
        });
        assert_eq!(measured, ["a", "b"]);
        assert_eq!(plan, vec![("a", 5), ("b", 5)]);
    }
}
