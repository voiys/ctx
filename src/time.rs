use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Offset, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

static TIME_STATE: LazyLock<Mutex<TimeState>> = LazyLock::new(|| Mutex::new(TimeState::default()));

#[derive(Clone, Debug)]
struct TimeState {
    zone: ResolvedTimeZone,
}

impl Default for TimeState {
    fn default() -> Self {
        Self {
            zone: ResolvedTimeZone::Iana("UTC".parse().expect("UTC timezone")),
        }
    }
}

#[derive(Clone, Debug)]
enum ResolvedTimeZone {
    Iana(Tz),
    Fixed(FixedOffset, String),
}

impl ResolvedTimeZone {
    fn name(&self) -> String {
        match self {
            Self::Iana(zone) => zone.name().to_string(),
            Self::Fixed(_, name) => name.clone(),
        }
    }

    fn offset_for(&self, instant: DateTime<Utc>) -> FixedOffset {
        match self {
            Self::Iana(zone) => instant.with_timezone(zone).offset().fix(),
            Self::Fixed(offset, _) => *offset,
        }
    }

    fn local_parts(&self, instant: DateTime<Utc>) -> LocalParts {
        match self {
            Self::Iana(zone) => {
                let local = instant.with_timezone(zone);
                LocalParts {
                    year: local.year(),
                    month: local.month(),
                    day: local.day(),
                    hour: local.hour(),
                    minute: local.minute(),
                    second: local.second(),
                }
            }
            Self::Fixed(offset, _) => {
                let local = instant.with_timezone(offset);
                LocalParts {
                    year: local.year(),
                    month: local.month(),
                    day: local.day(),
                    hour: local.hour(),
                    minute: local.minute(),
                    second: local.second(),
                }
            }
        }
    }

    fn start_of_local_day_ms(&self, instant: DateTime<Utc>) -> i64 {
        let parts = self.local_parts(instant);
        let date =
            NaiveDate::from_ymd_opt(parts.year, parts.month, parts.day).expect("valid local date");
        let midnight = date.and_hms_opt(0, 0, 0).expect("valid midnight");
        match self {
            Self::Iana(zone) => zone
                .from_local_datetime(&midnight)
                .earliest()
                .expect("local midnight exists")
                .with_timezone(&Utc)
                .timestamp_millis(),
            Self::Fixed(offset, _) => offset
                .from_local_datetime(&midnight)
                .single()
                .expect("fixed offset midnight exists")
                .with_timezone(&Utc)
                .timestamp_millis(),
        }
    }
}

#[derive(Debug)]
struct LocalParts {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

pub(crate) fn init_time_module(timezone: Option<&str>) {
    let zone = resolve_time_zone(timezone);
    TIME_STATE.lock().expect("time state lock").zone = zone;
}

pub(crate) fn get_active_time_zone() -> String {
    TIME_STATE.lock().expect("time state lock").zone.name()
}

pub(crate) fn now_instant_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub(crate) fn format_local_date(instant: DateTime<Utc>) -> String {
    let zone = TIME_STATE.lock().expect("time state lock").zone.clone();
    let parts = zone.local_parts(instant);
    format!("{:04}-{:02}-{:02}", parts.year, parts.month, parts.day)
}

pub(crate) fn format_local_date_time(instant: DateTime<Utc>) -> String {
    let zone = TIME_STATE.lock().expect("time state lock").zone.clone();
    let parts = zone.local_parts(instant);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        parts.year, parts.month, parts.day, parts.hour, parts.minute, parts.second
    )
}

pub(crate) fn start_of_local_day(instant: DateTime<Utc>) -> i64 {
    let zone = TIME_STATE.lock().expect("time state lock").zone.clone();
    zone.start_of_local_day_ms(instant)
}

pub(crate) fn format_for_llm(input: impl Into<TimeInput>) -> String {
    let input = input.into();
    let original = input.original();
    let Some(instant) = input.into_instant() else {
        return original;
    };
    let zone = TIME_STATE.lock().expect("time state lock").zone.clone();
    let parts = zone.local_parts(instant);
    let offset = zone.offset_for(instant);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}",
        parts.year,
        parts.month,
        parts.day,
        parts.hour,
        parts.minute,
        parts.second,
        format_offset(offset)
    )
}

pub(crate) fn describe_time_zone_for_prompt() -> String {
    let zone = TIME_STATE.lock().expect("time state lock").zone.clone();
    let name = zone.name();
    let offset = format_offset(zone.offset_for(Utc::now()));
    format!(
        "All timestamps below are in {name} (UTC{offset}). When reasoning about \"yesterday\", \"last week\", or time differences, use this timezone."
    )
}

pub(crate) fn reset_time_module_for_test() {
    init_time_module(Some("UTC"));
}

pub(crate) enum TimeInput {
    Instant(DateTime<Utc>),
    String(String),
    Millis(i64),
}

impl TimeInput {
    fn original(&self) -> String {
        match self {
            Self::Instant(instant) => instant.to_rfc3339(),
            Self::String(value) => value.clone(),
            Self::Millis(value) => value.to_string(),
        }
    }

    fn into_instant(self) -> Option<DateTime<Utc>> {
        match self {
            Self::Instant(instant) => Some(instant),
            Self::String(value) => DateTime::parse_from_rfc3339(&value)
                .map(|instant| instant.with_timezone(&Utc))
                .ok(),
            Self::Millis(value) => Utc.timestamp_millis_opt(value).single(),
        }
    }
}

impl From<DateTime<Utc>> for TimeInput {
    fn from(value: DateTime<Utc>) -> Self {
        Self::Instant(value)
    }
}

impl From<&str> for TimeInput {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<String> for TimeInput {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<i64> for TimeInput {
    fn from(value: i64) -> Self {
        Self::Millis(value)
    }
}

fn resolve_time_zone(value: Option<&str>) -> ResolvedTimeZone {
    let requested = value.unwrap_or("system");
    if requested.is_empty() || requested == "system" {
        let system = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string());
        return parse_time_zone(&system).unwrap_or_else(utc_zone);
    }
    parse_time_zone(requested).unwrap_or_else(|| {
        let system = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string());
        parse_time_zone(&system).unwrap_or_else(utc_zone)
    })
}

fn parse_time_zone(value: &str) -> Option<ResolvedTimeZone> {
    if let Some(offset) = parse_offset(value) {
        return Some(ResolvedTimeZone::Fixed(offset, value.to_string()));
    }
    value.parse::<Tz>().ok().map(ResolvedTimeZone::Iana)
}

fn utc_zone() -> ResolvedTimeZone {
    ResolvedTimeZone::Iana("UTC".parse().expect("UTC timezone"))
}

fn parse_offset(value: &str) -> Option<FixedOffset> {
    if value.len() != 6 {
        return None;
    }
    let sign = match &value[0..1] {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    if &value[3..4] != ":" {
        return None;
    }
    let hours = value[1..3].parse::<i32>().ok()?;
    let minutes = value[4..6].parse::<i32>().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))
}

fn format_offset(offset: FixedOffset) -> String {
    let seconds = offset.local_minus_utc();
    let sign = if seconds >= 0 { '+' } else { '-' };
    let abs = seconds.abs();
    format!("{sign}{:02}:{:02}", abs / 3600, (abs % 3600) / 60)
}

#[cfg(test)]
mod tests {
    use std::sync::{LazyLock, Mutex};

    use regex::Regex;

    use super::*;

    static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn instant(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn resolves_system_and_known_timezones() {
        let _guard = TEST_LOCK.lock().unwrap();
        init_time_module(None);
        assert!(!get_active_time_zone().is_empty());
        init_time_module(Some("system"));
        assert!(!get_active_time_zone().is_empty());
        init_time_module(Some("Asia/Shanghai"));
        assert_eq!(get_active_time_zone(), "Asia/Shanghai");
        init_time_module(Some("Europe/London"));
        assert_eq!(get_active_time_zone(), "Europe/London");
        init_time_module(Some("America/New_York"));
        assert_eq!(get_active_time_zone(), "America/New_York");
        init_time_module(Some("UTC"));
        assert_eq!(get_active_time_zone(), "UTC");
    }

    #[test]
    fn accepts_utc_offset_strings() {
        let _guard = TEST_LOCK.lock().unwrap();
        for zone in ["+08:00", "-05:00", "+05:30", "+09:30", "+00:00"] {
            init_time_module(Some(zone));
            assert_eq!(get_active_time_zone(), zone);
        }
    }

    #[test]
    fn falls_back_for_invalid_timezone() {
        let _guard = TEST_LOCK.lock().unwrap();
        init_time_module(Some("Invalid/FakeZone"));
        assert!(!get_active_time_zone().is_empty());
        init_time_module(Some(""));
        assert!(!get_active_time_zone().is_empty());
        init_time_module(Some("!!garbage!!"));
        assert!(!get_active_time_zone().is_empty());
    }

    #[test]
    fn reset_time_module_sets_utc() {
        let _guard = TEST_LOCK.lock().unwrap();
        init_time_module(Some("Asia/Tokyo"));
        assert_eq!(get_active_time_zone(), "Asia/Tokyo");
        reset_time_module_for_test();
        assert_eq!(get_active_time_zone(), "UTC");
    }

    #[test]
    fn now_instant_iso_returns_utc_milliseconds() {
        let _guard = TEST_LOCK.lock().unwrap();
        let value = now_instant_iso();
        assert!(
            Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$")
                .unwrap()
                .is_match(&value)
        );
    }

    #[test]
    fn local_date_respects_timezone_and_offsets() {
        let _guard = TEST_LOCK.lock().unwrap();
        init_time_module(Some("Asia/Shanghai"));
        assert_eq!(
            format_local_date(instant("2026-06-01T20:00:00Z")),
            "2026-06-02"
        );

        let value = instant("2026-01-01T03:00:00Z");
        init_time_module(Some("UTC"));
        assert_eq!(format_local_date(value), "2026-01-01");
        init_time_module(Some("Asia/Shanghai"));
        assert_eq!(format_local_date(value), "2026-01-01");
        init_time_module(Some("America/New_York"));
        assert_eq!(format_local_date(value), "2025-12-31");

        init_time_module(Some("Europe/London"));
        assert_eq!(
            format_local_date(instant("2026-03-29T00:30:00Z")),
            "2026-03-29"
        );
        assert_eq!(
            format_local_date(instant("2026-03-29T01:30:00Z")),
            "2026-03-29"
        );

        init_time_module(Some("+05:30"));
        assert_eq!(
            format_local_date(instant("2026-01-01T18:30:00Z")),
            "2026-01-02"
        );
    }

    #[test]
    fn local_date_time_formats_seconds() {
        let _guard = TEST_LOCK.lock().unwrap();
        init_time_module(Some("Asia/Shanghai"));
        assert_eq!(
            format_local_date_time(instant("2026-03-15T06:30:45Z")),
            "2026-03-15 14:30:45"
        );
        init_time_module(Some("UTC"));
        assert_eq!(
            format_local_date_time(instant("2026-06-01T00:00:00Z")),
            "2026-06-01 00:00:00"
        );
        assert_eq!(
            format_local_date_time(instant("2026-06-01T23:59:59Z")),
            "2026-06-01 23:59:59"
        );
    }

    #[test]
    fn start_of_local_day_matches_configured_timezone() {
        let _guard = TEST_LOCK.lock().unwrap();
        init_time_module(Some("Asia/Shanghai"));
        assert_eq!(
            start_of_local_day(instant("2026-06-02T04:00:00Z")),
            instant("2026-06-01T16:00:00Z").timestamp_millis()
        );
        init_time_module(Some("UTC"));
        assert_eq!(
            start_of_local_day(instant("2026-03-15T10:30:00Z")),
            instant("2026-03-15T00:00:00Z").timestamp_millis()
        );
        init_time_module(Some("America/New_York"));
        assert_eq!(
            start_of_local_day(instant("2026-01-15T10:00:00Z")),
            instant("2026-01-15T05:00:00Z").timestamp_millis()
        );
    }

    #[test]
    fn format_for_llm_supports_dates_strings_millis_and_offsets() {
        let _guard = TEST_LOCK.lock().unwrap();
        init_time_module(Some("Asia/Shanghai"));
        assert_eq!(
            format_for_llm(instant("2026-04-07T03:04:45Z")),
            "2026-04-07T11:04:45+08:00"
        );
        assert_eq!(
            format_for_llm("2026-04-07T03:04:45.000Z"),
            "2026-04-07T11:04:45+08:00"
        );

        init_time_module(Some("UTC"));
        let millis = instant("2026-06-01T12:00:00Z").timestamp_millis();
        assert_eq!(format_for_llm(millis), "2026-06-01T12:00:00+00:00");

        init_time_module(Some("America/New_York"));
        assert_eq!(
            format_for_llm("2026-01-15T17:30:00Z"),
            "2026-01-15T12:30:00-05:00"
        );
        assert_eq!(
            format_for_llm("2026-07-15T17:30:00Z"),
            "2026-07-15T13:30:00-04:00"
        );

        init_time_module(Some("+05:30"));
        assert_eq!(
            format_for_llm("2026-01-01T00:00:00Z"),
            "2026-01-01T05:30:00+05:30"
        );

        init_time_module(Some("UTC"));
        assert_eq!(format_for_llm("not-a-date"), "not-a-date");
        init_time_module(Some("Europe/Berlin"));
        assert_eq!(
            format_for_llm("2025-12-15T22:00:00.000Z"),
            "2025-12-15T23:00:00+01:00"
        );
    }

    #[test]
    fn describe_timezone_for_prompt_includes_name_offset_and_guidance() {
        let _guard = TEST_LOCK.lock().unwrap();
        init_time_module(Some("Asia/Shanghai"));
        let desc = describe_time_zone_for_prompt();
        assert!(desc.contains("Asia/Shanghai"));
        assert!(desc.contains("+08:00"));
        assert!(desc.contains("timestamps"));

        init_time_module(Some("UTC"));
        let desc = describe_time_zone_for_prompt();
        assert!(desc.contains("UTC"));
        assert!(desc.contains("+00:00"));

        init_time_module(Some("+05:30"));
        assert!(describe_time_zone_for_prompt().contains("+05:30"));
    }
}
