//! RFC 5545 subset recurrence expansion with DST-correct local→UTC conversion.
//! Supported: FREQ=DAILY|WEEKLY|MONTHLY, INTERVAL, BYDAY (weekly), BYMONTHDAY (monthly),
//! COUNT, UNTIL (local date, inclusive). Anything else is rejected, never silently ignored.
//! Occurrences are generated in the rule's local wall time; a local time in a spring-forward gap
//! shifts forward and an ambiguous fall-back time resolves to the earlier instant ("compatible").
use jiff::civil::{Date, DateTime, Time, Weekday};
use jiff::tz::TimeZone;
use jiff::ToSpan;

const MAX_ITERATIONS: usize = 20_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub freq: Freq,
    pub interval: i32,
    pub by_day: Vec<Weekday>,
    pub by_month_day: Option<i8>,
    pub count: Option<u32>,
    pub until: Option<Date>,
}

/// Local start of a series: a wall-clock date-time, or a date for all-day series.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Start {
    pub date: Date,
    pub time: Option<Time>,
}

pub fn parse_start(s: &str) -> Result<Start, String> {
    if s.contains('T') {
        let dt: DateTime = s
            .parse()
            .map_err(|_| format!("invalid local date-time: {s}"))?;
        Ok(Start {
            date: dt.date(),
            time: Some(dt.time()),
        })
    } else {
        let d: Date = s.parse().map_err(|_| format!("invalid local date: {s}"))?;
        Ok(Start {
            date: d,
            time: None,
        })
    }
}

pub fn format_local(date: Date, time: Option<Time>) -> String {
    match time {
        Some(t) => DateTime::from_parts(date, t).to_string(),
        None => date.to_string(),
    }
}

pub fn tz(name: &str) -> Result<TimeZone, String> {
    TimeZone::get(name).map_err(|_| format!("unknown time zone: {name}"))
}

/// Local wall time → UTC instant (RFC 3339, `Z`), resolving gaps/folds as documented above.
pub fn local_to_utc(local: &str, tz_name: &str) -> Result<String, String> {
    let dt: DateTime = local
        .parse()
        .map_err(|_| format!("invalid local date-time: {local}"))?;
    let z = dt.to_zoned(tz(tz_name)?).map_err(|e| e.to_string())?;
    Ok(z.timestamp().strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// UTC instant → local wall time in `tz_name` (no offset suffix).
pub fn utc_to_local(utc: &str, tz_name: &str) -> Result<String, String> {
    let ts: jiff::Timestamp = utc.parse().map_err(|_| format!("invalid instant: {utc}"))?;
    Ok(ts.to_zoned(tz(tz_name)?).datetime().to_string())
}

fn weekday_code(s: &str) -> Option<Weekday> {
    Some(match s {
        "MO" => Weekday::Monday,
        "TU" => Weekday::Tuesday,
        "WE" => Weekday::Wednesday,
        "TH" => Weekday::Thursday,
        "FR" => Weekday::Friday,
        "SA" => Weekday::Saturday,
        "SU" => Weekday::Sunday,
        _ => return None,
    })
}

pub fn parse_rule(rrule: &str) -> Result<Rule, String> {
    let mut freq = None;
    let mut rule = Rule {
        freq: Freq::Daily,
        interval: 1,
        by_day: vec![],
        by_month_day: None,
        count: None,
        until: None,
    };
    for part in rrule
        .trim_start_matches("RRULE:")
        .split(';')
        .filter(|p| !p.is_empty())
    {
        let (k, v) = part
            .split_once('=')
            .ok_or_else(|| format!("malformed rule part: {part}"))?;
        match k {
            "FREQ" => {
                freq = Some(match v {
                    "DAILY" => Freq::Daily,
                    "WEEKLY" => Freq::Weekly,
                    "MONTHLY" => Freq::Monthly,
                    _ => return Err(format!("unsupported FREQ: {v}")),
                })
            }
            "INTERVAL" => {
                rule.interval = v
                    .parse()
                    .ok()
                    .filter(|n| (1..=366).contains(n))
                    .ok_or("INTERVAL must be 1..366")?
            }
            "BYDAY" => {
                for d in v.split(',') {
                    rule.by_day
                        .push(weekday_code(d).ok_or_else(|| format!("unsupported BYDAY: {d}"))?);
                }
            }
            "BYMONTHDAY" => {
                rule.by_month_day = Some(
                    v.parse()
                        .ok()
                        .filter(|n| (1..=31).contains(n))
                        .ok_or("BYMONTHDAY must be 1..31")?,
                )
            }
            "COUNT" => {
                rule.count = Some(
                    v.parse()
                        .ok()
                        .filter(|n| *n >= 1)
                        .ok_or("COUNT must be >= 1")?,
                )
            }
            "UNTIL" => {
                let d = v
                    .get(..8)
                    .filter(|_| v.len() == 8 || v.ends_with('Z'))
                    .ok_or("UNTIL must be YYYYMMDD")?;
                let iso = format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..8]);
                rule.until = Some(iso.parse().map_err(|_| "invalid UNTIL")?);
            }
            other => return Err(format!("unsupported rule part: {other}")),
        }
    }
    rule.freq = freq.ok_or("FREQ is required")?;
    if rule.count.is_some() && rule.until.is_some() {
        return Err("COUNT and UNTIL are mutually exclusive".into());
    }
    if !rule.by_day.is_empty() && rule.freq != Freq::Weekly {
        return Err("BYDAY is only supported with FREQ=WEEKLY".into());
    }
    if rule.by_month_day.is_some() && rule.freq != Freq::Monthly {
        return Err("BYMONTHDAY is only supported with FREQ=MONTHLY".into());
    }
    rule.by_day.sort_by_key(|d| d.to_monday_zero_offset());
    rule.by_day.dedup();
    Ok(rule)
}

/// Local dates of occurrences within `[from, to]` (inclusive), COUNT/UNTIL applied from the series start.
pub fn expand(rule: &Rule, start: Date, from: Date, to: Date) -> Result<Vec<Date>, String> {
    let mut out = vec![];
    let mut emitted: u32 = 0;
    let mut iterations = 0usize;
    let limit = rule.until.map_or(to, |u| u.min(to));
    let mut push = |d: Date, out: &mut Vec<Date>| -> bool {
        // Returns false when generation must stop.
        if d < start {
            return true;
        }
        if d > limit {
            return false;
        }
        emitted += 1;
        if rule.count.is_some_and(|c| emitted > c) {
            return false;
        }
        if d >= from {
            out.push(d);
        }
        true
    };
    match rule.freq {
        Freq::Daily => {
            let mut k = 0i32;
            loop {
                iterations += 1;
                if iterations > MAX_ITERATIONS {
                    return Err("recurrence expansion limit exceeded".into());
                }
                let d = start
                    .checked_add((k * rule.interval).days())
                    .map_err(|e| e.to_string())?;
                if !push(d, &mut out) {
                    break;
                }
                k += 1;
            }
        }
        Freq::Weekly => {
            let days: Vec<Weekday> = if rule.by_day.is_empty() {
                vec![start.weekday()]
            } else {
                rule.by_day.clone()
            };
            let week0 = start
                .checked_sub((start.weekday().to_monday_zero_offset() as i32).days())
                .map_err(|e| e.to_string())?;
            let mut w = 0i32;
            'weeks: loop {
                let monday = week0
                    .checked_add((7 * w * rule.interval).days())
                    .map_err(|e| e.to_string())?;
                if monday > limit {
                    break;
                }
                for wd in &days {
                    iterations += 1;
                    if iterations > MAX_ITERATIONS {
                        return Err("recurrence expansion limit exceeded".into());
                    }
                    let d = monday
                        .checked_add((wd.to_monday_zero_offset() as i32).days())
                        .map_err(|e| e.to_string())?;
                    if !push(d, &mut out) {
                        break 'weeks;
                    }
                }
                w += 1;
            }
        }
        Freq::Monthly => {
            let day = rule.by_month_day.unwrap_or(start.day());
            let mut k = 0i32;
            loop {
                iterations += 1;
                if iterations > MAX_ITERATIONS {
                    return Err("recurrence expansion limit exceeded".into());
                }
                let first = start
                    .first_of_month()
                    .checked_add((k * rule.interval).months())
                    .map_err(|e| e.to_string())?;
                if first > limit {
                    break;
                }
                // Months lacking the requested day are skipped (RFC 5545), never clamped.
                if let Ok(d) = Date::new(first.year(), first.month(), day) {
                    if !push(d, &mut out) {
                        break;
                    }
                }
                k += 1;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        s.parse().unwrap()
    }
    fn dates(rrule: &str, start: &str, from: &str, to: &str) -> Vec<String> {
        expand(&parse_rule(rrule).unwrap(), d(start), d(from), d(to))
            .unwrap()
            .iter()
            .map(|x| x.to_string())
            .collect()
    }

    #[test]
    fn daily_interval_and_count() {
        assert_eq!(
            dates(
                "FREQ=DAILY;INTERVAL=2;COUNT=3",
                "2026-10-19",
                "2026-10-01",
                "2026-12-31"
            ),
            ["2026-10-19", "2026-10-21", "2026-10-23"]
        );
    }

    #[test]
    fn count_is_counted_from_series_start_not_window() {
        assert_eq!(
            dates(
                "FREQ=DAILY;COUNT=3",
                "2026-10-19",
                "2026-10-21",
                "2026-10-30"
            ),
            ["2026-10-21"]
        );
    }

    #[test]
    fn weekly_byday_and_until() {
        assert_eq!(
            dates(
                "FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20261028",
                "2026-10-19",
                "2026-10-19",
                "2026-12-31"
            ),
            ["2026-10-19", "2026-10-21", "2026-10-26", "2026-10-28"]
        );
    }

    #[test]
    fn weekly_interval_two() {
        assert_eq!(
            dates(
                "FREQ=WEEKLY;INTERVAL=2;COUNT=3",
                "2026-10-19",
                "2026-10-01",
                "2027-01-01"
            ),
            ["2026-10-19", "2026-11-02", "2026-11-16"]
        );
    }

    #[test]
    fn monthly_skips_months_without_the_day() {
        assert_eq!(
            dates(
                "FREQ=MONTHLY;COUNT=4",
                "2026-01-31",
                "2026-01-01",
                "2027-01-01"
            ),
            ["2026-01-31", "2026-03-31", "2026-05-31", "2026-07-31"]
        );
        assert_eq!(
            dates(
                "FREQ=MONTHLY;BYMONTHDAY=29",
                "2027-12-01",
                "2028-01-01",
                "2028-03-31"
            ),
            ["2028-01-29", "2028-02-29", "2028-03-29"]
        );
    }

    #[test]
    fn rejects_unsupported_and_conflicting_rules() {
        for bad in [
            "FREQ=YEARLY",
            "FREQ=DAILY;BYSETPOS=1",
            "FREQ=DAILY;COUNT=2;UNTIL=20270101",
            "FREQ=DAILY;BYDAY=MO",
            "INTERVAL=2",
            "FREQ=DAILY;INTERVAL=0",
            "FREQ=WEEKLY;BYDAY=XX",
        ] {
            assert!(parse_rule(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn berlin_spring_forward_gap_shifts_forward() {
        // 2026-03-29 02:30 does not exist in Europe/Berlin (02:00 -> 03:00).
        assert_eq!(
            local_to_utc("2026-03-28T02:30:00", "Europe/Berlin").unwrap(),
            "2026-03-28T01:30:00Z"
        );
        assert_eq!(
            local_to_utc("2026-03-29T02:30:00", "Europe/Berlin").unwrap(),
            "2026-03-29T01:30:00Z"
        );
        assert_eq!(
            local_to_utc("2026-03-30T02:30:00", "Europe/Berlin").unwrap(),
            "2026-03-30T00:30:00Z"
        );
    }

    #[test]
    fn berlin_fall_back_fold_takes_earlier_instant() {
        // 2026-10-25 02:30 occurs twice; the earlier one is still CEST (UTC+2).
        assert_eq!(
            local_to_utc("2026-10-25T02:30:00", "Europe/Berlin").unwrap(),
            "2026-10-25T00:30:00Z"
        );
        assert_eq!(
            local_to_utc("2026-10-26T02:30:00", "Europe/Berlin").unwrap(),
            "2026-10-26T01:30:00Z"
        );
    }

    #[test]
    fn weekly_wall_time_is_stable_across_dst() {
        let r = parse_rule("FREQ=WEEKLY;BYDAY=MO").unwrap();
        let days = expand(&r, d("2026-03-23"), d("2026-03-23"), d("2026-03-30")).unwrap();
        let utc: Vec<String> = days
            .iter()
            .map(|x| local_to_utc(&format!("{x}T09:00:00"), "Europe/Berlin").unwrap())
            .collect();
        assert_eq!(utc, ["2026-03-23T08:00:00Z", "2026-03-30T07:00:00Z"]);
    }

    #[test]
    fn utc_round_trips_to_local_and_rejects_bad_zone() {
        assert_eq!(
            utc_to_local("2026-10-25T00:30:00Z", "Europe/Berlin").unwrap(),
            "2026-10-25T02:30:00"
        );
        assert!(tz("Mars/Olympus").is_err());
    }
}
