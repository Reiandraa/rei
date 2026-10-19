//! Deterministic deadline date/time resolution (docs/UI-PLANNER-DATES-NEXT.md §A).
//!
//! The model only copies literal wording ("bis Freitag um 12 Uhr"). The date is computed
//! here from the source's ORIGINAL timestamp and the configured IANA zone — never from the
//! fetch/processing time and never from model arithmetic. Source content cannot choose the
//! zone. Anything not clearly supported stays `ambiguous` for the owner to resolve.
//!
//! Documented interpretation rules:
//! - heute/today/tonight = source day; morgen/tomorrow = +1; übermorgen/day after tomorrow = +2.
//!   "heute morgen", "am Morgen", "guten Morgen" are not "tomorrow".
//! - A bare or "kommenden"/"coming" weekday is the upcoming occurrence within 1..7 days after
//!   the source day. If the source was sent ON that weekday, it means the same day only when a
//!   clock time is stated that is still later than the send time; otherwise it is ambiguous
//!   (cautious rule: never silently roll to next week).
//! - "diesen/this <weekday>" and "diese Woche/this week" = that day of the source's ISO week
//!   (Mon-Sun); already past = ambiguous. "nächste Woche/next week" + weekday = that day of the
//!   following ISO week. "nächsten Freitag"/"next Friday" alone is genuinely ambiguous.
//! - Several different dates or clock times, time ranges, day/month without a year, recurring
//!   wording ("freitags"), forwarded messages (unknown original anchor) and wall times that do
//!   not exist or occur twice at a DST change are ambiguous.
//! - A missing clock time keeps the deadline date-only (never midnight).
use jiff::{civil, tz::TimeZone, Timestamp, ToSpan};

/// Bumped when the rules change so eligible pending results can be upgraded once.
pub const RESOLVER_VERSION: i64 = 2;
/// The owner's zone (docs/OWNER-DECISIONS.md); configurable in Automatic assistance.
pub const DEFAULT_ZONE: &str = "Europe/Berlin";
pub const NO_CLEAR_DATE: &str = "the wording does not name one clear date";

#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// YYYY-MM-DD in the zone.
    pub due_date: Option<String>,
    /// HH:MM wall time in the zone; None = date-only deadline.
    pub due_time: Option<String>,
    /// none | explicit | ambiguous
    pub status: &'static str,
    /// written (full date in the text) | relative (computed from the source timestamp)
    pub basis: Option<&'static str>,
    /// Short English explanation shown with the result.
    pub note: String,
}

impl Resolution {
    pub fn none() -> Self {
        Resolution {
            due_date: None,
            due_time: None,
            status: "none",
            basis: None,
            note: String::new(),
        }
    }
    pub fn unclear(note: impl Into<String>) -> Self {
        Resolution {
            due_date: None,
            due_time: None,
            status: "ambiguous",
            basis: None,
            note: note.into(),
        }
    }
}

pub fn check_zone(zone: &str) -> Result<TimeZone, String> {
    if zone.is_empty() || zone.len() > 64 {
        return Err("time zone is not valid".into());
    }
    TimeZone::get(zone).map_err(|_| format!("unknown time zone: {zone}"))
}

/// Wording that means the visible text is a forward, so its own timestamp is not the anchor.
pub fn looks_forwarded(visible_text: &str) -> bool {
    let l = visible_text.to_lowercase();
    let first = l.lines().next().unwrap_or("");
    let subj = first.strip_prefix("subject:").unwrap_or("").trim_start();
    ["fwd:", "fw:", "wg:", "wtr:"]
        .iter()
        .any(|p| subj.starts_with(p))
        || [
            "weitergeleitete nachricht",
            "forwarded message",
            "original message",
            "ursprüngliche nachricht",
        ]
        .iter()
        .any(|p| l.contains(p))
}

const WEEKDAYS: &[(&str, i8)] = &[
    ("montag", 1),
    ("monday", 1),
    ("dienstag", 2),
    ("tuesday", 2),
    ("mittwoch", 3),
    ("wednesday", 3),
    ("donnerstag", 4),
    ("thursday", 4),
    ("freitag", 5),
    ("friday", 5),
    ("samstag", 6),
    ("sonnabend", 6),
    ("saturday", 6),
    ("sonntag", 7),
    ("sunday", 7),
];
const RECURRING: &[&str] = &[
    "montags",
    "dienstags",
    "mittwochs",
    "donnerstags",
    "freitags",
    "samstags",
    "sonntags",
    "mondays",
    "tuesdays",
    "wednesdays",
    "thursdays",
    "fridays",
    "saturdays",
    "sundays",
];

fn words(lower: &str) -> Vec<String> {
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Clock times written in the text, as (hour, minute). Err = a range or invalid time.
fn clock_times(lower: &str) -> Result<Vec<(i8, i8)>, String> {
    let c: Vec<char> = lower.chars().collect();
    let n = c.len();
    let mut out: Vec<(i8, i8)> = Vec::new();
    let mut i = 0;
    let skip_ws = |mut j: usize| {
        while j < n && c[j] == ' ' {
            j += 1;
        }
        j
    };
    let word_at = |j: usize, w: &str| -> bool {
        let wc: Vec<char> = w.chars().collect();
        j + wc.len() <= n
            && c[j..j + wc.len()] == wc[..]
            && (j + wc.len() == n || !c[j + wc.len()].is_alphanumeric())
    };
    while i < n {
        if !c[i].is_ascii_digit()
            || (i > 0 && (c[i - 1].is_alphanumeric() || c[i - 1] == '.' || c[i - 1] == ':'))
        {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n && c[j].is_ascii_digit() {
            j += 1;
        }
        if j - i > 2 {
            i = j;
            continue;
        }
        let h: i32 = c[i..j].iter().collect::<String>().parse().unwrap_or(99);
        let mut m: i32 = 0;
        let mut k = j;
        let mut has_min = false;
        if k + 2 < n
            && (c[k] == ':' || c[k] == '.')
            && c[k + 1].is_ascii_digit()
            && c[k + 2].is_ascii_digit()
            && (k + 3 >= n || !c[k + 3].is_ascii_digit())
        {
            // "14.11.2026"/"23.10." are dates, not times.
            let is_date = c[k] == '.' && k + 3 < n && c[k + 3] == '.';
            if !is_date {
                m = c[k + 1..k + 3]
                    .iter()
                    .collect::<String>()
                    .parse()
                    .unwrap_or(99);
                has_min = true;
                k += 3;
            }
        }
        let t = skip_ws(k);
        let uhr = word_at(t, "uhr") || word_at(t, "h");
        let pm = word_at(t, "pm") || (word_at(t, "p") && t + 1 < n && c[t + 1] == '.');
        // German "12 am Freitag" is not 12 a.m.; only a.m./am at a phrase end counts.
        let am = (word_at(t, "a") && t + 1 < n && c[t + 1] == '.')
            || (word_at(t, "am") && {
                let e = skip_ws(t + 2);
                e >= n || !c[e].is_alphabetic()
            });
        let dotted_needs_uhr = has_min && c[j] == '.';
        let accepted = if pm || am {
            true
        } else if has_min {
            !dotted_needs_uhr || uhr
        } else {
            uhr
        };
        if accepted {
            let mut hh = h;
            if pm || am {
                if !(1..=12).contains(&h) {
                    return Err("clock time is not valid".into());
                }
                hh = if pm {
                    if h == 12 {
                        12
                    } else {
                        h + 12
                    }
                } else if h == 12 {
                    0
                } else {
                    h
                };
            }
            if !(0..=23).contains(&hh) || !(0..=59).contains(&m) {
                return Err("clock time is not valid".into());
            }
            // "10-12 Uhr" / "10 bis 12 Uhr": a range is not one deadline time.
            let mut b = i;
            while b > 0 && c[b - 1] == ' ' {
                b -= 1;
            }
            if b > 0 && matches!(c[b - 1], '-' | '–') {
                let mut d = b - 1;
                while d > 0 && c[d - 1] == ' ' {
                    d -= 1;
                }
                if d > 0 && c[d - 1].is_ascii_digit() {
                    return Err("a time range was given".into());
                }
            }
            let before: String = c[..i].iter().collect();
            let bw = words(&before);
            // "10 bis 12 Uhr", "10 oder 12 Uhr", "9 or 10 am": a range or a choice.
            if bw.len() >= 2
                && matches!(
                    bw[bw.len() - 1].as_str(),
                    "bis" | "oder" | "or" | "to" | "und" | "and"
                )
                && bw[bw.len() - 2].chars().all(|x| x.is_ascii_digit())
            {
                return Err("a time range or choice was given".into());
            }
            let v = (hh as i8, m as i8);
            if !out.contains(&v) {
                out.push(v);
            }
        }
        i = k.max(j);
    }
    Ok(out)
}

/// "23.10." or "23/10" without a year.
fn has_partial_date(lower: &str) -> bool {
    let c: Vec<char> = lower.chars().collect();
    let n = c.len();
    let mut i = 0;
    while i < n {
        if c[i].is_ascii_digit() && (i == 0 || !c[i - 1].is_alphanumeric() && c[i - 1] != '.') {
            let mut j = i;
            while j < n && c[j].is_ascii_digit() {
                j += 1;
            }
            if j - i <= 2 && j < n && (c[j] == '.' || c[j] == '/') {
                let mut k = j + 1;
                while k < n && c[k].is_ascii_digit() {
                    k += 1;
                }
                if (1..=2).contains(&(k - j - 1)) {
                    // followed by a year -> full date, handled elsewhere
                    let mut y = k;
                    if y < n && (c[y] == '.' || c[y] == '/') {
                        y += 1;
                    }
                    let ys = y;
                    while y < n && c[y].is_ascii_digit() {
                        y += 1;
                    }
                    let is_time = c[j] == '.' && k < n && !(c[k] == '.' || c[k] == '/');
                    if y - ys != 4 && !is_time && k < n && (c[k] == '.' || c[k] == '/') {
                        return true;
                    }
                }
                i = k;
                continue;
            }
            i = j;
            continue;
        }
        i += 1;
    }
    false
}

/// Resolves the literal date wording `text` against the source timestamp `anchor` in `zone`.
/// `model_due` is the model's own YYYY-MM-DD claim; it can only confirm a written date.
pub fn resolve(
    text: &str,
    anchor: Option<&str>,
    zone: &str,
    forwarded: bool,
    model_due: Option<&str>,
) -> Resolution {
    let lower = text.to_lowercase();
    let w = words(&lower);
    let written = crate::inbox::explicit_dates(text);
    let times = match clock_times(&lower) {
        Ok(t) => t,
        Err(e) => return Resolution::unclear(e),
    };
    if times.len() > 1 {
        return Resolution::unclear("more than one clock time is mentioned");
    }
    if w.iter().any(|x| RECURRING.contains(&x.as_str())) {
        return Resolution::unclear("recurring wording, not one deadline");
    }
    // Relative references found in the wording.
    let has = |s: &str| w.iter().any(|x| x == s);
    let seq = |a: &str, b: &str| w.windows(2).any(|p| p[0] == a && p[1] == b);
    let seq3 =
        |a: &str, b: &str, cc: &str| w.windows(3).any(|p| p[0] == a && p[1] == b && p[2] == cc);
    let mut offsets: Vec<i64> = Vec::new();
    if has("heute") || has("today") || has("tonight") || seq("this", "evening") {
        offsets.push(0);
    }
    if has("übermorgen") || has("uebermorgen") || seq3("day", "after", "tomorrow") {
        offsets.push(2);
    }
    if has("tomorrow") && !seq3("day", "after", "tomorrow") {
        offsets.push(1);
    }
    for (idx, x) in w.iter().enumerate() {
        if x == "morgen" {
            let prev = if idx > 0 { w[idx - 1].as_str() } else { "" };
            if !matches!(
                prev,
                "am" | "jeden" | "heute" | "guten" | "diesen" | "gestern"
            ) {
                offsets.push(1);
            }
        }
    }
    let next_week = seq("nächste", "woche")
        || seq("naechste", "woche")
        || seq("kommende", "woche")
        || seq("next", "week")
        || seq("nächster", "woche")
        || seq("folgenden", "woche")
        || seq("following", "week");
    let this_week = seq("diese", "woche") || seq("dieser", "woche") || seq("this", "week");
    let mut weekdays: Vec<(i8, &'static str)> = Vec::new();
    for (idx, x) in w.iter().enumerate() {
        if let Some((_, d)) = WEEKDAYS.iter().find(|(name, _)| *name == x.as_str()) {
            let prev = if idx > 0 { w[idx - 1].as_str() } else { "" };
            let mode = match prev {
                "nächsten" | "nächste" | "nächster" | "naechsten" | "next" | "übernächsten" => {
                    if next_week {
                        "next_week"
                    } else {
                        "unclear_next"
                    }
                }
                "diesen" | "dieser" | "diese" | "this" => "this_week",
                _ if next_week => "next_week",
                _ if this_week => "this_week",
                _ => "upcoming",
            };
            weekdays.push((*d, mode));
        }
    }
    let relative = !offsets.is_empty() || !weekdays.is_empty();
    if written.is_empty() && !relative {
        if has_partial_date(&lower) {
            return Resolution::unclear("day and month are given without a year");
        }
        if next_week || this_week || !times.is_empty() || !text.trim().is_empty() {
            return Resolution::unclear(NO_CLEAR_DATE);
        }
        return Resolution::none();
    }
    if has_partial_date(&lower) {
        return Resolution::unclear("day and month are given without a year");
    }
    let mut candidates: Vec<civil::Date> = Vec::new();
    let mut basis = "written";
    if written.len() > 1 {
        return Resolution::unclear("more than one date is mentioned");
    }
    if let Some(d) = written.first() {
        if let Some(md) = model_due {
            if md.len() != 10 || md.parse::<civil::Date>().ok() != Some(*d) {
                return Resolution::unclear("the model's date does not match the written date");
            }
        }
        candidates.push(*d);
    }
    let local_time = times.first().map(|(h, m)| civil::time(*h, *m, 0, 0));
    // "Freitag, 30.10.2026": with a written full date the weekday only has to agree with it;
    // it is not re-anchored to the send day (that would wrongly pick the upcoming Friday).
    if let Some(d) = written.first() {
        let wdn = d.weekday().to_monday_one_offset();
        if weekdays.iter().any(|(t, _)| *t != wdn) {
            return Resolution::unclear("the weekday does not match the written date");
        }
        weekdays.clear();
    }
    let relative = !offsets.is_empty() || !weekdays.is_empty();
    if relative {
        if forwarded {
            return Resolution::unclear("forwarded message: the original send date is unknown");
        }
        let Some(a) = anchor.and_then(|a| a.parse::<Timestamp>().ok()) else {
            return Resolution::unclear("the message has no valid send time");
        };
        let Ok(tz) = check_zone(zone) else {
            return Resolution::unclear("the configured time zone is not valid");
        };
        let local = a.to_zoned(tz);
        let day = local.date();
        let wd = day.weekday().to_monday_one_offset();
        for o in &offsets {
            match day.checked_add(o.days()) {
                Ok(d) => candidates.push(d),
                Err(_) => return Resolution::unclear("date out of range"),
            }
        }
        for (target, mode) in &weekdays {
            let monday = day.checked_sub(((wd - 1) as i64).days());
            let d = match *mode {
                "unclear_next" => {
                    return Resolution::unclear("\"next <weekday>\" can mean this or next week")
                }
                "next_week" => {
                    monday.and_then(|m| m.checked_add(((*target - 1) as i64 + 7).days()))
                }
                "this_week" => {
                    let d = monday.and_then(|m| m.checked_add(((*target - 1) as i64).days()));
                    match d {
                        Ok(x) if x < day => {
                            return Resolution::unclear("that weekday has already passed this week")
                        }
                        other => other,
                    }
                }
                _ => {
                    let delta = (*target - wd).rem_euclid(7) as i64;
                    day.checked_add(delta.days())
                }
            };
            let Ok(d) = d else {
                return Resolution::unclear("date out of range");
            };
            if d == day {
                // Sent on the named weekday: same day only if a stated time is still ahead.
                let ahead = local_time.map(|t| t > local.time()).unwrap_or(false);
                let explicit_today = offsets.contains(&0);
                if !ahead && !explicit_today {
                    return Resolution::unclear(
                        "sent on that weekday; today or next week is unclear",
                    );
                }
            }
            candidates.push(d);
        }
        if written.is_empty() {
            basis = "relative";
        }
    }
    candidates.dedup();
    let first = candidates[0];
    if candidates.iter().any(|d| *d != first) {
        return Resolution::unclear("the wording names different dates");
    }
    let mut note = if basis == "relative" {
        format!("Interpreted from the message's send date in {zone}")
    } else {
        String::from("Written in the message")
    };
    let due_time = match local_time {
        None => None,
        Some(t) => {
            let Ok(tz) = check_zone(zone) else {
                return Resolution::unclear("the configured time zone is not valid");
            };
            if tz.to_ambiguous_zoned(first.to_datetime(t)).is_ambiguous() {
                return Resolution::unclear(
                    "that clock time is skipped or repeated by a daylight-saving change",
                );
            }
            if basis == "written" {
                note = format!("Written in the message; time in {zone}");
            }
            Some(format!("{:02}:{:02}", t.hour(), t.minute()))
        }
    };
    Resolution {
        due_date: Some(first.to_string()),
        due_time,
        status: "explicit",
        basis: Some(basis),
        note,
    }
}

/// UTC instant for a resolved wall-clock deadline (refuses DST gaps/folds).
pub fn to_utc(date: &str, time: &str, zone: &str) -> Result<String, String> {
    let d: civil::Date = date
        .parse()
        .map_err(|_| "due date must be YYYY-MM-DD".to_string())?;
    let t: civil::Time = time
        .parse()
        .map_err(|_| "due time must be HH:MM".to_string())?;
    if time.len() != 5 {
        return Err("due time must be HH:MM".into());
    }
    let tz = check_zone(zone)?;
    let amb = tz.to_ambiguous_zoned(d.to_datetime(t));
    if amb.is_ambiguous() {
        return Err("that clock time is skipped or repeated by a daylight-saving change".into());
    }
    amb.unambiguous()
        .map(|z| z.timestamp().to_string())
        .map_err(|_| "time out of range".to_string())
}
