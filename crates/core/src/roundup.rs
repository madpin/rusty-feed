//! Shared, personal-data-free rules for feed and folder Roundups.
//!
//! A Roundup is generated once for an ordered-set-independent hash of its input
//! articles. Selection and key construction live here so the web request and
//! worker always address the same shared row.

use std::collections::HashSet;

use time::{Duration, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset, Weekday};
use time_tz::{timezones, OffsetDateTimeExt, OffsetResult, PrimitiveDateTimeExt};
use uuid::Uuid;

use crate::hash::{hex_encode, sha256_bytes};

/// Bump this whenever the Roundup instructions or output contract changes.
pub const ROUNDUP_PROMPT_VERSION: &str = "roundup-v1";
/// Minimum number of recent articles needed for a useful cohesive article.
pub const ROUNDUP_MIN_ARTICLES: usize = 3;
/// Maximum number of ranked articles sent to the provider.
pub const ROUNDUP_MAX_ARTICLES: usize = 30;
/// Recent candidates ranked in memory; only summaries/excerpts are selected.
pub const ROUNDUP_CANDIDATE_LIMIT: i64 = 120;
/// One Roundup costs five units against the funding-agnostic daily LLM call cap.
pub const ROUNDUP_LLM_CALL_WEIGHT: i64 = 5;
/// Default maximum number of repeating Roundups owned by one user.
pub const DEFAULT_ROUNDUP_SCHEDULE_LIMIT: u32 = 10;
/// Personal Roundup history is retained for ninety days.
pub const ROUNDUP_HISTORY_RETENTION_DAYS: u32 = 90;
/// Weekly Roundups use this local hour; daily Roundups choose their own hour.
pub const ROUNDUP_DEFAULT_LOCAL_HOUR: u8 = 7;

/// Return whether another schedule would exceed the configured per-user cap.
pub fn roundup_schedule_cap_reached(existing: u64, limit: u32) -> bool {
    existing >= u64::from(limit)
}

/// User-local timing for one repeating Roundup. ISO weekdays are Monday = 1
/// through Sunday = 7 in storage; `time::Weekday` is used at the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundupTiming {
    pub period: RoundupPeriod,
    pub local_hour: u8,
    pub weekday: Option<Weekday>,
    pub timezone: String,
}

impl RoundupTiming {
    /// Validate the persisted/form timing without silently accepting an unknown
    /// timezone or an impossible cadence.
    pub fn is_valid(&self) -> bool {
        self.local_hour <= 23
            && crate::i18n::is_supported_timezone(&self.timezone)
            && match self.period {
                RoundupPeriod::Day => self.weekday.is_none(),
                RoundupPeriod::Week => self.weekday.is_some(),
            }
    }

    /// First local-cadence instant strictly after `after`, returned in UTC.
    /// A spring-forward gap moves to the first valid local minute; an ambiguous
    /// autumn time uses its first occurrence, preventing a duplicate window.
    pub fn next_run_after(&self, after: OffsetDateTime) -> Option<OffsetDateTime> {
        if !self.is_valid() {
            return None;
        }
        let local_after = if self.timezone == "UTC" {
            after
        } else {
            after.to_timezone(timezones::get_by_name(&self.timezone)?)
        };
        let time = Time::from_hms(self.local_hour, 0, 0).ok()?;
        for day_offset in 0..=8 {
            let date = local_after.date() + Duration::days(day_offset);
            if self.period == RoundupPeriod::Week && Some(date.weekday()) != self.weekday {
                continue;
            }
            let local = PrimitiveDateTime::new(date, time);
            if let Some(candidate) = resolve_local(local, &self.timezone) {
                if candidate > after {
                    return Some(candidate);
                }
            }
        }
        None
    }
}

/// Convert an ISO weekday number used by forms and PostgreSQL.
pub fn roundup_weekday(value: u8) -> Option<Weekday> {
    match value {
        1 => Some(Weekday::Monday),
        2 => Some(Weekday::Tuesday),
        3 => Some(Weekday::Wednesday),
        4 => Some(Weekday::Thursday),
        5 => Some(Weekday::Friday),
        6 => Some(Weekday::Saturday),
        7 => Some(Weekday::Sunday),
        _ => None,
    }
}

fn resolve_local(local: PrimitiveDateTime, timezone: &str) -> Option<OffsetDateTime> {
    if timezone == "UTC" {
        return Some(local.assume_utc());
    }
    let zone = timezones::get_by_name(timezone)?;
    // Most gaps are one hour; a bounded minute walk also handles unusual
    // thirty-minute transitions without looping over invalid local time.
    for minute in 0..=180 {
        let candidate = local + Duration::minutes(minute);
        match candidate.assume_timezone(zone) {
            OffsetResult::Some(value) => return Some(value.to_offset(UtcOffset::UTC)),
            OffsetResult::Ambiguous(first, second) => {
                return Some(first.min(second).to_offset(UtcOffset::UTC));
            }
            OffsetResult::None => {}
        }
    }
    None
}

/// The rolling input window for a Roundup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoundupPeriod {
    Day,
    Week,
}

impl RoundupPeriod {
    /// Parse the public form/query value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            _ => None,
        }
    }

    /// Stable database, cache-key, and form value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
        }
    }

    /// Rolling window length: exactly 24 hours or seven days.
    pub fn window(self) -> Duration {
        match self {
            Self::Day => Duration::hours(24),
            Self::Week => Duration::days(7),
        }
    }

    /// Whole hours used by the candidate query.
    pub fn window_hours(self) -> i64 {
        self.window().whole_hours()
    }
}

/// The complete shared identity of one Roundup generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundupKey {
    pub input_hash: String,
    pub period: RoundupPeriod,
    pub model_version: String,
}

impl RoundupKey {
    /// Hash the sorted, de-duplicated input IDs and retain the model/prompt
    /// dimensions separately for the shared table's unique constraint.
    pub fn new(article_ids: &[Uuid], period: RoundupPeriod, model_version: &str) -> Self {
        Self {
            input_hash: input_hash(article_ids),
            period,
            model_version: model_version.to_string(),
        }
    }

    /// Compact queue key covering every shared-table uniqueness dimension.
    pub fn dedup_key(&self) -> String {
        let material = format!(
            "{}|{}|{}|{}",
            self.input_hash,
            self.period.as_str(),
            ROUNDUP_PROMPT_VERSION,
            self.model_version
        );
        format!("roundup:{}", hex_encode(&sha256_bytes(material.as_bytes())))
    }
}

/// Lowercase SHA-256 of sorted, de-duplicated article UUID bytes.
pub fn input_hash(article_ids: &[Uuid]) -> String {
    let mut ids = article_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut bytes = Vec::with_capacity(ids.len() * 16);
    for id in ids {
        bytes.extend_from_slice(id.as_bytes());
    }
    hex_encode(&sha256_bytes(&bytes))
}

/// Shared article fields eligible for selection and prompt construction.
/// Deliberately has no personal feed name, folder, read state, or preference.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundupCandidate {
    pub id: Uuid,
    pub title: String,
    pub feed_title: String,
    pub url: String,
    pub summary_or_excerpt: String,
    pub published_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub story_count: i32,
    pub has_summary: bool,
}

impl RoundupCandidate {
    fn sort_timestamp(&self) -> OffsetDateTime {
        self.published_at.unwrap_or(self.created_at)
    }

    /// Importance from shared story coverage (25 points per extra source,
    /// capped at ten), recency (up to 20), and an existing AI summary (10).
    pub fn importance(&self, now: OffsetDateTime, period: RoundupPeriod) -> f64 {
        let coverage = f64::from(self.story_count.clamp(1, 10) - 1) * 25.0;
        let summary = if self.has_summary { 10.0 } else { 0.0 };
        let age = now - self.sort_timestamp();
        let freshness =
            (1.0 - age.as_seconds_f64() / period.window().as_seconds_f64()).clamp(0.0, 1.0);
        coverage + summary + 20.0 * freshness
    }
}

/// Rank the bounded candidate set and keep at most 30 distinct articles.
pub fn select_candidates(
    mut candidates: Vec<RoundupCandidate>,
    period: RoundupPeriod,
    now: OffsetDateTime,
) -> Vec<RoundupCandidate> {
    candidates.sort_by(|left, right| {
        right
            .importance(now, period)
            .total_cmp(&left.importance(now, period))
            .then_with(|| right.sort_timestamp().cmp(&left.sort_timestamp()))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.id));
    candidates.truncate(ROUNDUP_MAX_ARTICLES);
    candidates
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn candidate(id: Uuid, story_count: i32, has_summary: bool, hour: u8) -> RoundupCandidate {
        let published_at = datetime!(2026-11-17 12:00 UTC) + Duration::hours(i64::from(hour));
        RoundupCandidate {
            id,
            title: format!("Article {id}"),
            feed_title: "Shared feed".into(),
            url: format!("https://example.com/{id}"),
            summary_or_excerpt: "Shared summary".into(),
            published_at: Some(published_at),
            created_at: published_at,
            story_count,
            has_summary,
        }
    }

    #[test]
    fn key_hash_is_order_independent_and_covers_every_dimension() {
        let first = Uuid::now_v7();
        let second = Uuid::now_v7();
        let forward = RoundupKey::new(&[first, second, first], RoundupPeriod::Day, "model-a");
        let reversed = RoundupKey::new(&[second, first], RoundupPeriod::Day, "model-a");
        assert_eq!(forward, reversed);
        assert_eq!(forward.input_hash.len(), 64);
        assert_ne!(
            forward.dedup_key(),
            RoundupKey::new(&[first, second], RoundupPeriod::Week, "model-a").dedup_key()
        );
        assert_ne!(
            forward.dedup_key(),
            RoundupKey::new(&[first, second], RoundupPeriod::Day, "model-b").dedup_key()
        );
        assert_ne!(
            forward.input_hash,
            input_hash(&[first, second, Uuid::now_v7()])
        );
    }

    #[test]
    fn selection_prioritizes_shared_signals_and_caps_at_thirty() {
        let now = datetime!(2026-11-18 12:00 UTC);
        let story = Uuid::now_v7();
        let summarized = Uuid::now_v7();
        let recent = Uuid::now_v7();
        let old = Uuid::now_v7();
        let candidates = vec![
            candidate(old, 1, false, 0),
            candidate(recent, 1, false, 23),
            candidate(summarized, 1, true, 12),
            candidate(story, 3, false, 0),
        ];
        let selected = select_candidates(candidates, RoundupPeriod::Day, now);
        let ids: Vec<_> = selected.iter().map(|candidate| candidate.id).collect();
        assert_eq!(ids, vec![story, summarized, recent, old]);

        let many = (0..40)
            .map(|_| candidate(Uuid::now_v7(), 1, false, 12))
            .collect();
        assert_eq!(
            select_candidates(many, RoundupPeriod::Day, now).len(),
            ROUNDUP_MAX_ARTICLES
        );
    }

    #[test]
    fn periods_are_exact_rolling_windows() {
        assert_eq!(RoundupPeriod::Day.window(), Duration::hours(24));
        assert_eq!(RoundupPeriod::Week.window(), Duration::days(7));
        assert_eq!(RoundupPeriod::parse("day"), Some(RoundupPeriod::Day));
        assert_eq!(RoundupPeriod::parse("week"), Some(RoundupPeriod::Week));
        assert_eq!(RoundupPeriod::parse("month"), None);
    }

    #[test]
    fn next_run_uses_local_days_and_iso_weekdays() {
        let daily = RoundupTiming {
            period: RoundupPeriod::Day,
            local_hour: 7,
            weekday: None,
            timezone: "Europe/Dublin".into(),
        };
        assert_eq!(
            daily.next_run_after(datetime!(2026-11-17 06:30 UTC)),
            Some(datetime!(2026-11-17 07:00 UTC))
        );
        assert_eq!(
            daily.next_run_after(datetime!(2026-11-17 07:00 UTC)),
            Some(datetime!(2026-11-18 07:00 UTC))
        );
        let weekly = RoundupTiming {
            period: RoundupPeriod::Week,
            local_hour: 7,
            weekday: Some(Weekday::Monday),
            timezone: "Europe/Dublin".into(),
        };
        assert_eq!(
            weekly.next_run_after(datetime!(2026-11-17 12:00 UTC)),
            Some(datetime!(2026-11-23 07:00 UTC))
        );
        assert_eq!(roundup_weekday(1), Some(Weekday::Monday));
        assert_eq!(roundup_weekday(7), Some(Weekday::Sunday));
        assert_eq!(roundup_weekday(0), None);
        assert!(!roundup_schedule_cap_reached(9, 10));
        assert!(roundup_schedule_cap_reached(10, 10));
    }

    #[test]
    fn next_run_is_safe_across_dst_gaps_and_ambiguous_times() {
        let spring = RoundupTiming {
            period: RoundupPeriod::Week,
            local_hour: 2,
            weekday: Some(Weekday::Sunday),
            timezone: "Europe/Berlin".into(),
        };
        // 02:00 does not exist on this date; the first valid instant is 03:00.
        assert_eq!(
            spring.next_run_after(datetime!(2026-03-28 12:00 UTC)),
            Some(datetime!(2026-03-29 01:00 UTC))
        );
        let autumn = RoundupTiming {
            local_hour: 2,
            ..spring
        };
        // 02:00 occurs twice; use the first occurrence, not both.
        assert_eq!(
            autumn.next_run_after(datetime!(2026-10-24 12:00 UTC)),
            Some(datetime!(2026-10-25 00:00 UTC))
        );
        assert_eq!(
            autumn.next_run_after(datetime!(2026-10-25 00:00 UTC)),
            Some(datetime!(2026-11-01 01:00 UTC))
        );
    }

    #[test]
    fn invalid_timing_never_produces_a_run() {
        let timing = RoundupTiming {
            period: RoundupPeriod::Day,
            local_hour: 24,
            weekday: None,
            timezone: "UTC".into(),
        };
        assert_eq!(timing.next_run_after(OffsetDateTime::now_utc()), None);
        let timing = RoundupTiming {
            local_hour: 7,
            timezone: "Not/A_Zone".into(),
            ..timing
        };
        assert_eq!(timing.next_run_after(OffsetDateTime::now_utc()), None);
    }
}
