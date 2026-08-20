//! Date prototype and static method dispatch.
//!
//! Contains `dispatch_date_method` for routing method calls on Date objects,
//! `dispatch_date_static_method_with_args` for `Date.parse` and `Date.UTC`,
//! and helper functions for date component extraction and formatting.

use nanbox::JsValue;

use crate::internal_data::{InternalData, InternalKind, UnifiedObject};
use crate::tagged_obj::{deref_tagged, deref_tagged_mut};

use super::{make_rt_string, read_argv};

// =========================================================================
// Constants
// =========================================================================

/// Milliseconds per second.
const MS_PER_SECOND: f64 = 1000.0;
/// Milliseconds per minute.
const MS_PER_MINUTE: f64 = 60_000.0;
/// Milliseconds per hour.
const MS_PER_HOUR: f64 = 3_600_000.0;
/// Milliseconds per day.
const MS_PER_DAY: f64 = 86_400_000.0;

/// Day names for toString formatting.
const DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Month names for toString formatting.
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

// =========================================================================
// Date component extraction (UTC)
// =========================================================================

/// Extract year/month/day/hours/minutes/seconds/ms from a UTC timestamp.
///
/// Returns `None` if the timestamp is NaN or infinite.
fn utc_components(ms: f64) -> Option<(i32, u32, u32, u32, u32, u32, u32)> {
    if !ms.is_finite() {
        return None;
    }
    let ms_i64 = ms as i64;
    let day_ms = ms_i64.rem_euclid(MS_PER_DAY as i64);
    let hours = (day_ms / MS_PER_HOUR as i64) as u32;
    let minutes = ((day_ms % MS_PER_HOUR as i64) / MS_PER_MINUTE as i64) as u32;
    let seconds = ((day_ms % MS_PER_MINUTE as i64) / MS_PER_SECOND as i64) as u32;
    let millis = (day_ms % MS_PER_SECOND as i64) as u32;

    let days = (ms_i64 as f64 / MS_PER_DAY).floor() as i64;
    let (year, month, day) = days_to_ymd(days);

    Some((year, month, day, hours, minutes, seconds, millis))
}

/// Convert days since epoch (1970-01-01) to (year, month 0-11, day 1-31).
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    // Algorithm from https://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // year of era [0, 399]
    let y = (yoe as i64 + era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m - 1, d) // month is 0-indexed for JS compatibility
}

/// Convert (year, month 0-11, day 1-31) to days since epoch (1970-01-01).
fn ymd_to_days(year: i32, month: u32, day: u32) -> i64 {
    // Adjust for 0-indexed month to 1-indexed
    let m = month + 1;
    let y = if m <= 2 { year as i64 - 1 } else { year as i64 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32; // year of era [0, 399]
    let m_adj = if m > 2 { m - 3 } else { m + 9 }; // adjusted month [0, 11]
    let doy = (153 * m_adj + 2) / 5 + day - 1; // day of year [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // day of era [0, 146096]
    era * 146097 + doe as i64 - 719468
}

/// Day of week (0=Sunday, 6=Saturday) from days since epoch.
fn day_of_week(ms: f64) -> u32 {
    if !ms.is_finite() {
        return 0;
    }
    let days = (ms / MS_PER_DAY).floor() as i64;
    ((days + 4).rem_euclid(7)) as u32 // 1970-01-01 was Thursday (4)
}

/// Get the local timezone offset in milliseconds for a specific timestamp.
///
/// This accounts for DST changes. Returns 0 in test mode.
fn local_tz_offset_ms(_timestamp: f64) -> f64 {
    // For now, return 0 (UTC). Full local time support requires platform-specific
    // timezone database access. The methods still work correctly for UTC.
    // TODO("v0.9: implement platform-specific timezone offset for local time methods")
    0.0
}

/// Convert UTC timestamp to local timestamp.
fn utc_to_local(ms: f64) -> f64 {
    if !ms.is_finite() {
        return f64::NAN;
    }
    ms + local_tz_offset_ms(ms)
}

/// Convert local timestamp to UTC timestamp.
pub(crate) fn local_to_utc(ms: f64) -> f64 {
    if !ms.is_finite() {
        return f64::NAN;
    }
    ms - local_tz_offset_ms(ms)
}

// =========================================================================
// Date construction helpers
// =========================================================================

/// Build a UTC timestamp from components (year, month, ...).
///
/// `year` follows JS rules: 0-99 maps to 1900-1999.
/// `month` is 0-indexed (0=January, 11=December).
/// Extract a numeric value from a JsValue, handling both float and int NaN-boxing.
fn to_number_or(v: &JsValue, default: f64) -> f64 {
    if let Some(n) = v.as_number() {
        n
    } else if let Some(i) = v.as_int() {
        i as f64
    } else {
        default
    }
}

pub(crate) fn make_date_from_components(args: &[JsValue]) -> f64 {
    let year_raw = args.first().map_or(f64::NAN, |v| to_number_or(v, f64::NAN));
    if year_raw.is_nan() {
        return f64::NAN;
    }
    let mut year = year_raw as i32;
    // ES spec: if 0 <= year <= 99, add 1900
    if (0..=99).contains(&year) {
        year += 1900;
    }

    let month = args.get(1).map_or(0.0, |v| to_number_or(v, 0.0)) as i32;
    let day = args.get(2).map_or(1.0, |v| to_number_or(v, 1.0)) as u32;
    let hours = args.get(3).map_or(0.0, |v| to_number_or(v, 0.0)) as u32;
    let minutes = args.get(4).map_or(0.0, |v| to_number_or(v, 0.0)) as u32;
    let seconds = args.get(5).map_or(0.0, |v| to_number_or(v, 0.0)) as u32;
    let millis = args.get(6).map_or(0.0, |v| to_number_or(v, 0.0)) as u32;

    // Handle month overflow/underflow (e.g., month=13 -> next year Jan)
    let adjusted_year = year + month.div_euclid(12);
    let adjusted_month = month.rem_euclid(12) as u32;

    let days = ymd_to_days(adjusted_year, adjusted_month, day);
    days as f64 * MS_PER_DAY
        + hours as f64 * MS_PER_HOUR
        + minutes as f64 * MS_PER_MINUTE
        + seconds as f64 * MS_PER_SECOND
        + millis as f64
}

/// Build a UTC timestamp from components for `Date.UTC()`.
///
/// Same as `make_date_from_components` but always UTC (no local adjustment).
pub(crate) fn make_utc_from_components(args: &[JsValue]) -> f64 {
    make_date_from_components(args)
}

/// Parse a date string (ISO 8601 subset) into a millisecond timestamp.
///
/// Supports formats like:
/// - `2024-03-15T10:30:00.000Z`
/// - `2024-03-15T10:30:00Z`
/// - `2024-03-15`
/// - `2024-03`
/// - `2024`
///
/// Returns `NaN` if the string cannot be parsed.
pub(crate) fn parse_date_string(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return f64::NAN;
    }

    // Try ISO 8601 format: YYYY-MM-DDTHH:MM:SS.sssZ
    if let Some(result) = try_parse_iso(s) {
        return result;
    }

    // Try simpler formats
    if let Some(result) = try_parse_date_only(s) {
        return result;
    }

    f64::NAN
}

/// Try to parse an ISO 8601 date string.
fn try_parse_iso(s: &str) -> Option<f64> {
    // Handle optional 'Z' or timezone offset at the end
    let (datetime_part, tz_offset_ms) = if let Some(rest) = s.strip_suffix('Z') {
        (rest, 0.0)
    } else if let Some(plus_idx) = s.rfind('+') {
        if plus_idx > 10 {
            // After the date portion
            let offset = parse_tz_offset(&s[plus_idx..])?;
            (&s[..plus_idx], offset)
        } else {
            (s, 0.0)
        }
    } else if let Some(minus_idx) = s.rfind('-') {
        if minus_idx > 10 {
            // After the date portion, likely timezone offset
            let offset = parse_tz_offset(&s[minus_idx..])?;
            (&s[..minus_idx], offset)
        } else {
            (s, 0.0)
        }
    } else {
        (s, 0.0)
    };

    let parts: Vec<&str> = datetime_part.split('T').collect();
    let date_part = parts.first()?;
    let time_part = parts.get(1).copied();

    let date_components: Vec<&str> = date_part.split('-').collect();
    let year: i32 = date_components.first()?.parse().ok()?;
    let month: u32 = date_components
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let day: u32 = date_components
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let (hours, minutes, seconds, millis) = if let Some(time) = time_part {
        parse_time_components(time)?
    } else {
        (0, 0, 0, 0)
    };

    let days = ymd_to_days(year, month - 1, day); // month is 1-indexed in ISO, convert to 0-indexed
    let ms = days as f64 * MS_PER_DAY
        + hours as f64 * MS_PER_HOUR
        + minutes as f64 * MS_PER_MINUTE
        + seconds as f64 * MS_PER_SECOND
        + millis as f64
        - tz_offset_ms;

    Some(ms)
}

/// Parse time components from a time string (HH:MM:SS.sss).
fn parse_time_components(time: &str) -> Option<(u32, u32, u32, u32)> {
    let (time_no_ms, millis) = if let Some(dot_idx) = time.find('.') {
        let ms_str = &time[dot_idx + 1..];
        let ms: u32 = match ms_str.len() {
            1 => ms_str.parse::<u32>().ok()? * 100,
            2 => ms_str.parse::<u32>().ok()? * 10,
            3 => ms_str.parse().ok()?,
            _ => ms_str[..3].parse().ok()?,
        };
        (&time[..dot_idx], ms)
    } else {
        (time, 0)
    };

    let parts: Vec<&str> = time_no_ms.split(':').collect();
    let hours: u32 = parts.first()?.parse().ok()?;
    let minutes: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let seconds: u32 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);

    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }

    Some((hours, minutes, seconds, millis))
}

/// Try to parse a date-only string (YYYY, YYYY-MM, YYYY-MM-DD).
fn try_parse_date_only(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split('-').collect();
    let year: i32 = parts.first()?.parse().ok()?;
    let month: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
    let day: u32 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);

    // Validate only if it looks like a date with separators
    if parts.len() == 1 && s.len() != 4 {
        return None; // Must be exactly 4-digit year
    }

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let days = ymd_to_days(year, month - 1, day);
    Some(days as f64 * MS_PER_DAY)
}

/// Parse a timezone offset string like "+05:30" or "-08:00".
fn parse_tz_offset(s: &str) -> Option<f64> {
    let sign: f64 = if s.starts_with('+') { 1.0 } else { -1.0 };
    let offset_str = &s[1..];
    let parts: Vec<&str> = offset_str.split(':').collect();
    let hours: f64 = parts.first()?.parse().ok()?;
    let minutes: f64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    Some(sign * (hours * MS_PER_HOUR + minutes * MS_PER_MINUTE))
}

// =========================================================================
// Date method dispatch
// =========================================================================

/// Extract the timestamp from a Date object.
///
/// Returns `None` if the object is not a Date.
fn extract_timestamp(obj: u64) -> Option<f64> {
    let uni = unsafe {
        // SAFETY: caller verifies obj is a tagged unified object.
        deref_tagged::<UnifiedObject>(obj)
    }?;
    if uni.kind != InternalKind::DateObj {
        return None;
    }
    match uni.internal_data() {
        Some(InternalData::Date { timestamp }) => Some(*timestamp),
        _ => None,
    }
}

/// Set the timestamp on a Date object.
///
/// Returns `true` if the object is a Date and was updated.
fn set_timestamp(obj: u64, new_ts: f64) -> bool {
    let uni = unsafe {
        // SAFETY: caller verifies obj is a tagged unified object.
        deref_tagged_mut::<UnifiedObject>(obj)
    };
    let Some(u) = uni else { return false };
    if u.kind != InternalKind::DateObj {
        return false;
    }
    match u.internal_data_mut() {
        Some(InternalData::Date { timestamp }) => {
            *timestamp = new_ts;
            true
        }
        _ => false,
    }
}

/// Dispatch a Date instance method call.
///
/// Routes to the appropriate getter, setter, or formatting method based on
/// the method name string.
pub(crate) fn dispatch_date_method(obj: u64, method: &str, argc: u32, argv: *const u64) -> u64 {
    let args = read_argv(argc, argv);

    match method {
        // ---- Getters (UTC) ----
        "getTime" | "valueOf" => {
            let ts = extract_timestamp(obj).unwrap_or(f64::NAN);
            JsValue::number(ts).raw_bits()
        }
        "getFullYear" => get_local_component(obj, Component::Year),
        "getMonth" => get_local_component(obj, Component::Month),
        "getDate" => get_local_component(obj, Component::Day),
        "getDay" => get_local_component(obj, Component::DayOfWeek),
        "getHours" => get_local_component(obj, Component::Hours),
        "getMinutes" => get_local_component(obj, Component::Minutes),
        "getSeconds" => get_local_component(obj, Component::Seconds),
        "getMilliseconds" => get_local_component(obj, Component::Milliseconds),
        "getTimezoneOffset" => {
            let ts = extract_timestamp(obj).unwrap_or(f64::NAN);
            if ts.is_nan() {
                return JsValue::number(f64::NAN).raw_bits();
            }
            // getTimezoneOffset returns minutes, positive = west of UTC
            let offset_ms = local_tz_offset_ms(ts);
            JsValue::number(-offset_ms / MS_PER_MINUTE).raw_bits()
        }

        // ---- Getters (UTC) ----
        "getUTCFullYear" => get_utc_component(obj, Component::Year),
        "getUTCMonth" => get_utc_component(obj, Component::Month),
        "getUTCDate" => get_utc_component(obj, Component::Day),
        "getUTCDay" => get_utc_component(obj, Component::DayOfWeek),
        "getUTCHours" => get_utc_component(obj, Component::Hours),
        "getUTCMinutes" => get_utc_component(obj, Component::Minutes),
        "getUTCSeconds" => get_utc_component(obj, Component::Seconds),
        "getUTCMilliseconds" => get_utc_component(obj, Component::Milliseconds),

        // ---- Setters ----
        "setTime" => {
            let ms = args.first().and_then(|v| v.as_number()).unwrap_or(f64::NAN);
            set_timestamp(obj, ms);
            JsValue::number(ms).raw_bits()
        }
        "setMilliseconds" => set_local_component(obj, &args, SetTarget::Milliseconds),
        "setSeconds" => set_local_component(obj, &args, SetTarget::Seconds),
        "setMinutes" => set_local_component(obj, &args, SetTarget::Minutes),
        "setHours" => set_local_component(obj, &args, SetTarget::Hours),
        "setDate" => set_local_component(obj, &args, SetTarget::Date),
        "setMonth" => set_local_component(obj, &args, SetTarget::Month),
        "setFullYear" => set_local_component(obj, &args, SetTarget::FullYear),

        "setUTCMilliseconds" => set_utc_component(obj, &args, SetTarget::Milliseconds),
        "setUTCSeconds" => set_utc_component(obj, &args, SetTarget::Seconds),
        "setUTCMinutes" => set_utc_component(obj, &args, SetTarget::Minutes),
        "setUTCHours" => set_utc_component(obj, &args, SetTarget::Hours),
        "setUTCDate" => set_utc_component(obj, &args, SetTarget::Date),
        "setUTCMonth" => set_utc_component(obj, &args, SetTarget::Month),
        "setUTCFullYear" => set_utc_component(obj, &args, SetTarget::FullYear),

        // ---- Formatting ----
        "toString" => date_to_string(obj),
        "toDateString" => date_to_date_string(obj),
        "toTimeString" => date_to_time_string(obj),
        "toISOString" => date_to_iso_string(obj),
        "toUTCString" => date_to_utc_string(obj),
        "toJSON" => date_to_iso_string(obj), // toJSON calls toISOString per spec
        "toLocaleDateString" => date_to_date_string(obj), // simplified: same as toDateString
        "toLocaleTimeString" => date_to_time_string(obj), // simplified: same as toTimeString
        "toLocaleString" => date_to_string(obj), // simplified: same as toString

        // ---- Symbol.toPrimitive ----
        "@@toPrimitive" | "[Symbol.toPrimitive]" => {
            // hint is the first argument: "default", "string", or "number"
            let hint = args
                .first()
                .map(|v| crate::string_ops::get_string_data(*v))
                .unwrap_or_default();
            match hint.as_str() {
                "number" => {
                    let ts = extract_timestamp(obj).unwrap_or(f64::NAN);
                    JsValue::number(ts).raw_bits()
                }
                _ => date_to_string(obj), // "default" and "string" both return toString
            }
        }

        _ => JsValue::undefined().raw_bits(),
    }
}

// =========================================================================
// Component extraction
// =========================================================================

/// Which date component to extract.
enum Component {
    /// Full year (e.g., 2024).
    Year,
    /// Month (0-11).
    Month,
    /// Day of month (1-31).
    Day,
    /// Day of week (0=Sunday, 6=Saturday).
    DayOfWeek,
    /// Hours (0-23).
    Hours,
    /// Minutes (0-59).
    Minutes,
    /// Seconds (0-59).
    Seconds,
    /// Milliseconds (0-999).
    Milliseconds,
}

/// Get a local-time component from a Date object.
fn get_local_component(obj: u64, component: Component) -> u64 {
    let ts = extract_timestamp(obj).unwrap_or(f64::NAN);
    if ts.is_nan() {
        return JsValue::number(f64::NAN).raw_bits();
    }
    let local_ts = utc_to_local(ts);
    get_component_from_ts(local_ts, &component)
}

/// Get a UTC component from a Date object.
fn get_utc_component(obj: u64, component: Component) -> u64 {
    let ts = extract_timestamp(obj).unwrap_or(f64::NAN);
    if ts.is_nan() {
        return JsValue::number(f64::NAN).raw_bits();
    }
    get_component_from_ts(ts, &component)
}

/// Extract a specific component value from a timestamp.
fn get_component_from_ts(ts: f64, component: &Component) -> u64 {
    if matches!(component, Component::DayOfWeek) {
        return JsValue::number(day_of_week(ts) as f64).raw_bits();
    }

    let Some((year, month, day, hours, minutes, seconds, millis)) = utc_components(ts) else {
        return JsValue::number(f64::NAN).raw_bits();
    };

    let val: f64 = match component {
        Component::Year => year as f64,
        Component::Month => month as f64,
        Component::Day => day as f64,
        Component::DayOfWeek => unreachable!(), // handled above
        Component::Hours => hours as f64,
        Component::Minutes => minutes as f64,
        Component::Seconds => seconds as f64,
        Component::Milliseconds => millis as f64,
    };
    JsValue::number(val).raw_bits()
}

// =========================================================================
// Component setting
// =========================================================================

/// Which date component to set.
enum SetTarget {
    /// setMilliseconds / setUTCMilliseconds
    Milliseconds,
    /// setSeconds / setUTCSeconds
    Seconds,
    /// setMinutes / setUTCMinutes
    Minutes,
    /// setHours / setUTCHours
    Hours,
    /// setDate / setUTCDate
    Date,
    /// setMonth / setUTCMonth
    Month,
    /// setFullYear / setUTCFullYear
    FullYear,
}

/// Set a local-time component on a Date object.
fn set_local_component(obj: u64, args: &[JsValue], target: SetTarget) -> u64 {
    let ts = extract_timestamp(obj).unwrap_or(f64::NAN);
    let local_ts = utc_to_local(ts);
    let new_local = apply_set(local_ts, args, target);
    let new_utc = local_to_utc(new_local);
    set_timestamp(obj, new_utc);
    JsValue::number(new_utc).raw_bits()
}

/// Set a UTC component on a Date object.
fn set_utc_component(obj: u64, args: &[JsValue], target: SetTarget) -> u64 {
    let ts = extract_timestamp(obj).unwrap_or(f64::NAN);
    let new_ts = apply_set(ts, args, target);
    set_timestamp(obj, new_ts);
    JsValue::number(new_ts).raw_bits()
}

/// Apply a setter operation to a timestamp, returning the new timestamp.
fn apply_set(ts: f64, args: &[JsValue], target: SetTarget) -> f64 {
    let val = args.first().and_then(|v| v.as_number()).unwrap_or(f64::NAN);
    if val.is_nan() || !ts.is_finite() {
        return f64::NAN;
    }

    let Some((year, month, day, hours, minutes, seconds, millis)) = utc_components(ts) else {
        return f64::NAN;
    };

    match target {
        SetTarget::Milliseconds => {
            let new_ms = val as u32;
            rebuild_timestamp(year, month, day, hours, minutes, seconds, new_ms)
        }
        SetTarget::Seconds => {
            let new_sec = val as u32;
            let new_ms = args
                .get(1)
                .and_then(|v| v.as_number())
                .map(|v| v as u32)
                .unwrap_or(millis);
            rebuild_timestamp(year, month, day, hours, minutes, new_sec, new_ms)
        }
        SetTarget::Minutes => {
            let new_min = val as u32;
            let new_sec = args
                .get(1)
                .and_then(|v| v.as_number())
                .map(|v| v as u32)
                .unwrap_or(seconds);
            let new_ms = args
                .get(2)
                .and_then(|v| v.as_number())
                .map(|v| v as u32)
                .unwrap_or(millis);
            rebuild_timestamp(year, month, day, hours, new_min, new_sec, new_ms)
        }
        SetTarget::Hours => {
            let new_hr = val as u32;
            let new_min = args
                .get(1)
                .and_then(|v| v.as_number())
                .map(|v| v as u32)
                .unwrap_or(minutes);
            let new_sec = args
                .get(2)
                .and_then(|v| v.as_number())
                .map(|v| v as u32)
                .unwrap_or(seconds);
            let new_ms = args
                .get(3)
                .and_then(|v| v.as_number())
                .map(|v| v as u32)
                .unwrap_or(millis);
            rebuild_timestamp(year, month, day, new_hr, new_min, new_sec, new_ms)
        }
        SetTarget::Date => {
            let new_day = val as u32;
            rebuild_timestamp(year, month, new_day, hours, minutes, seconds, millis)
        }
        SetTarget::Month => {
            let new_month = val as i32;
            let new_day = args
                .get(1)
                .and_then(|v| v.as_number())
                .map(|v| v as u32)
                .unwrap_or(day);
            // Handle month overflow/underflow
            let adj_year = year + new_month.div_euclid(12);
            let adj_month = new_month.rem_euclid(12) as u32;
            rebuild_timestamp(
                adj_year, adj_month, new_day, hours, minutes, seconds, millis,
            )
        }
        SetTarget::FullYear => {
            let new_year = val as i32;
            let new_month = args
                .get(1)
                .and_then(|v| v.as_number())
                .map(|v| v as i32)
                .unwrap_or(month as i32);
            let new_day = args
                .get(2)
                .and_then(|v| v.as_number())
                .map(|v| v as u32)
                .unwrap_or(day);
            // Handle month overflow/underflow
            let adj_year = new_year + new_month.div_euclid(12);
            let adj_month = new_month.rem_euclid(12) as u32;
            rebuild_timestamp(
                adj_year, adj_month, new_day, hours, minutes, seconds, millis,
            )
        }
    }
}

/// Rebuild a UTC timestamp from components.
fn rebuild_timestamp(
    year: i32,
    month: u32,
    day: u32,
    hours: u32,
    minutes: u32,
    seconds: u32,
    millis: u32,
) -> f64 {
    let days = ymd_to_days(year, month, day);
    days as f64 * MS_PER_DAY
        + hours as f64 * MS_PER_HOUR
        + minutes as f64 * MS_PER_MINUTE
        + seconds as f64 * MS_PER_SECOND
        + millis as f64
}

// =========================================================================
// Formatting
// =========================================================================

/// `Date.prototype.toString()` — full string representation.
fn date_to_string(obj: u64) -> u64 {
    let ts = extract_timestamp(obj).unwrap_or(f64::NAN);
    if !ts.is_finite() {
        return make_rt_string("Invalid Date".to_string());
    }
    let local_ts = utc_to_local(ts);
    let Some((year, month, day, hours, minutes, seconds, _millis)) = utc_components(local_ts)
    else {
        return make_rt_string("Invalid Date".to_string());
    };
    let dow = day_of_week(local_ts);
    let tz_offset = local_tz_offset_ms(ts);
    let tz_min = (-tz_offset / MS_PER_MINUTE) as i32;
    let tz_sign = if tz_min >= 0 { '+' } else { '-' };
    let tz_abs = tz_min.unsigned_abs();
    let tz_h = tz_abs / 60;
    let tz_m = tz_abs % 60;

    let s = format!(
        "{} {} {:02} {:04} {:02}:{:02}:{:02} GMT{}{:02}{:02}",
        DAY_NAMES[dow as usize],
        MONTH_NAMES[month as usize],
        day,
        year,
        hours,
        minutes,
        seconds,
        tz_sign,
        tz_h,
        tz_m,
    );
    make_rt_string(s)
}

/// `Date.prototype.toDateString()` — date portion only.
fn date_to_date_string(obj: u64) -> u64 {
    let ts = extract_timestamp(obj).unwrap_or(f64::NAN);
    if !ts.is_finite() {
        return make_rt_string("Invalid Date".to_string());
    }
    let local_ts = utc_to_local(ts);
    let Some((year, month, day, _h, _m, _s, _ms)) = utc_components(local_ts) else {
        return make_rt_string("Invalid Date".to_string());
    };
    let dow = day_of_week(local_ts);
    let s = format!(
        "{} {} {:02} {:04}",
        DAY_NAMES[dow as usize], MONTH_NAMES[month as usize], day, year,
    );
    make_rt_string(s)
}

/// `Date.prototype.toTimeString()` — time portion only.
fn date_to_time_string(obj: u64) -> u64 {
    let ts = extract_timestamp(obj).unwrap_or(f64::NAN);
    if !ts.is_finite() {
        return make_rt_string("Invalid Date".to_string());
    }
    let local_ts = utc_to_local(ts);
    let Some((_y, _mo, _d, hours, minutes, seconds, _ms)) = utc_components(local_ts) else {
        return make_rt_string("Invalid Date".to_string());
    };
    let tz_offset = local_tz_offset_ms(ts);
    let tz_min = (-tz_offset / MS_PER_MINUTE) as i32;
    let tz_sign = if tz_min >= 0 { '+' } else { '-' };
    let tz_abs = tz_min.unsigned_abs();
    let tz_h = tz_abs / 60;
    let tz_m = tz_abs % 60;
    let s = format!(
        "{:02}:{:02}:{:02} GMT{}{:02}{:02}",
        hours, minutes, seconds, tz_sign, tz_h, tz_m,
    );
    make_rt_string(s)
}

/// `Date.prototype.toISOString()` — ISO 8601 format.
fn date_to_iso_string(obj: u64) -> u64 {
    let ts = extract_timestamp(obj).unwrap_or(f64::NAN);
    if !ts.is_finite() {
        // Per spec, toISOString throws RangeError for invalid dates
        let msg = make_rt_string("Invalid time value".to_string());
        let err = super::__esc_rt_create_error(crate::exceptions::error_tag::RANGE_ERROR, msg);
        super::__esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    }
    let Some((year, month, day, hours, minutes, seconds, millis)) = utc_components(ts) else {
        let msg = make_rt_string("Invalid time value".to_string());
        let err = super::__esc_rt_create_error(crate::exceptions::error_tag::RANGE_ERROR, msg);
        super::__esc_rt_throw(err);
        return JsValue::undefined().raw_bits();
    };
    let s = if (0..=9999).contains(&year) {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            year,
            month + 1, // ISO month is 1-indexed
            day,
            hours,
            minutes,
            seconds,
            millis,
        )
    } else if year >= 0 {
        format!(
            "+{:06}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            year,
            month + 1,
            day,
            hours,
            minutes,
            seconds,
            millis,
        )
    } else {
        format!(
            "-{:06}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            -year,
            month + 1,
            day,
            hours,
            minutes,
            seconds,
            millis,
        )
    };
    make_rt_string(s)
}

/// `Date.prototype.toUTCString()` — RFC 7231 format.
fn date_to_utc_string(obj: u64) -> u64 {
    let ts = extract_timestamp(obj).unwrap_or(f64::NAN);
    if !ts.is_finite() {
        return make_rt_string("Invalid Date".to_string());
    }
    let Some((year, month, day, hours, minutes, seconds, _ms)) = utc_components(ts) else {
        return make_rt_string("Invalid Date".to_string());
    };
    let dow = day_of_week(ts);
    let s = format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        DAY_NAMES[dow as usize], day, MONTH_NAMES[month as usize], year, hours, minutes, seconds,
    );
    make_rt_string(s)
}
