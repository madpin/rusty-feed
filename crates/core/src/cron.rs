//! Small, dependency-free five-field cron matcher used by the durable worker.
//! Supports wildcards, lists, ranges, and steps with standard day-of-month /
//! day-of-week OR semantics.

use time::OffsetDateTime;

/// A validated five-field cron expression (`minute hour day month weekday`).
#[derive(Debug, Clone)]
pub struct CronSchedule {
    source: String,
    minutes: [bool; 60],
    hours: [bool; 24],
    days: [bool; 31],
    months: [bool; 12],
    weekdays: [bool; 7],
    day_wildcard: bool,
    weekday_wildcard: bool,
}

impl CronSchedule {
    /// Parse a standard five-field expression. Weekdays accept 0 or 7 for
    /// Sunday. Names and non-standard seconds/year fields are intentionally
    /// rejected so configuration remains portable and unambiguous.
    pub fn parse(source: &str) -> Result<Self, String> {
        let parts: Vec<_> = source.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(
                "cron expressions require five fields: minute hour day month weekday".into(),
            );
        }
        let (minutes, _) = parse_field(parts[0], 0, 59, false)?;
        let (hours, _) = parse_field(parts[1], 0, 23, false)?;
        let (days, day_wildcard) = parse_field(parts[2], 1, 31, false)?;
        let (months, _) = parse_field(parts[3], 1, 12, false)?;
        let (raw_weekdays, weekday_wildcard) = parse_field(parts[4], 0, 7, true)?;

        let mut schedule = Self {
            source: source.trim().to_string(),
            minutes: [false; 60],
            hours: [false; 24],
            days: [false; 31],
            months: [false; 12],
            weekdays: [false; 7],
            day_wildcard,
            weekday_wildcard,
        };
        copy_field(&minutes, &mut schedule.minutes, 0);
        copy_field(&hours, &mut schedule.hours, 0);
        copy_field(&days, &mut schedule.days, 1);
        copy_field(&months, &mut schedule.months, 1);
        for value in raw_weekdays {
            schedule.weekdays[if value == 7 { 0 } else { value }] = true;
        }
        Ok(schedule)
    }

    /// Original normalized expression.
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Whether this UTC minute is selected by the expression.
    pub fn matches(&self, instant: OffsetDateTime) -> bool {
        let minute = instant.minute() as usize;
        let hour = instant.hour() as usize;
        let day = instant.day() as usize - 1;
        let month = instant.month() as usize - 1;
        let weekday = instant.weekday().number_days_from_sunday() as usize;
        let day_match = self.days[day];
        let weekday_match = self.weekdays[weekday];
        let calendar_match = if self.day_wildcard || self.weekday_wildcard {
            day_match && weekday_match
        } else {
            day_match || weekday_match
        };
        self.minutes[minute] && self.hours[hour] && self.months[month] && calendar_match
    }
}

fn copy_field(values: &[usize], target: &mut [bool], minimum: usize) {
    for &value in values {
        target[value - minimum] = true;
    }
}

fn parse_field(
    source: &str,
    minimum: usize,
    maximum: usize,
    sunday_alias: bool,
) -> Result<(Vec<usize>, bool), String> {
    let wildcard = source == "*";
    let mut selected = vec![false; maximum + 1];
    for token in source.split(',') {
        if token.is_empty() {
            return Err(format!("invalid empty cron component in `{source}`"));
        }
        let (base, step) = match token.split_once('/') {
            Some((base, raw_step)) => {
                let step = raw_step
                    .parse::<usize>()
                    .map_err(|_| format!("invalid cron step `{raw_step}`"))?;
                if step == 0 {
                    return Err("cron steps must be greater than zero".into());
                }
                (base, step)
            }
            None => (token, 1),
        };
        let (start, end) = if base == "*" {
            (minimum, maximum)
        } else if let Some((start, end)) = base.split_once('-') {
            (
                parse_number(start, minimum, maximum)?,
                parse_number(end, minimum, maximum)?,
            )
        } else {
            let value = parse_number(base, minimum, maximum)?;
            (value, value)
        };
        if start > end {
            return Err(format!("cron range starts after it ends in `{token}`"));
        }
        for value in (start..=end).step_by(step) {
            selected[value] = true;
            if sunday_alias && value == 7 {
                selected[0] = true;
            }
        }
    }
    let values = (minimum..=maximum)
        .filter(|value| selected[*value])
        .collect();
    Ok((values, wildcard))
}

fn parse_number(source: &str, minimum: usize, maximum: usize) -> Result<usize, String> {
    let value = source
        .parse::<usize>()
        .map_err(|_| format!("invalid cron number `{source}`"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!(
            "cron number `{value}` is outside {minimum}..={maximum}"
        ));
    }
    Ok(value)
}

impl Default for CronSchedule {
    fn default() -> Self {
        Self::parse("*/5 * * * *").unwrap_or_else(|_| unreachable!("static cron is valid"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn matches_wildcards_steps_ranges_and_lists() {
        let schedule = CronSchedule::parse("*/15 8-18 * * 1-5").unwrap();
        assert!(schedule.matches(datetime!(2026-08-31 08:30 UTC)));
        assert!(!schedule.matches(datetime!(2026-08-30 08:30 UTC)));
        assert!(!schedule.matches(datetime!(2026-08-31 08:31 UTC)));
        let list = CronSchedule::parse("0 3 1,15 1,6 *").unwrap();
        assert!(list.matches(datetime!(2026-06-15 03:00 UTC)));
    }

    #[test]
    fn day_and_weekday_follow_cron_or_semantics() {
        let schedule = CronSchedule::parse("0 0 1 * 1").unwrap();
        assert!(schedule.matches(datetime!(2026-09-01 00:00 UTC)));
        assert!(schedule.matches(datetime!(2026-09-07 00:00 UTC)));
        assert!(!schedule.matches(datetime!(2026-09-02 00:00 UTC)));
    }

    #[test]
    fn rejects_ambiguous_or_out_of_range_expressions() {
        for invalid in [
            "",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "*/0 * * * *",
            "* * 9-2 * *",
        ] {
            assert!(CronSchedule::parse(invalid).is_err(), "{invalid}");
        }
    }
}
