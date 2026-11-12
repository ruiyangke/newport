//! User-facing dates for Files, Services and journal output.
//!
//! macOS uses Foundation's current locale, calendar and time zone, including the
//! user's 12/24-hour preference. The retained formatters are thread-local and
//! rebuilt after a locale/time-zone notification; rendering a journal does not
//! create a formatter for every row. Other platforms use explicit ISO-like local
//! dates and a 24-hour clock until a native locale adapter is available.

const MISSING: &str = "—";
// Bound times to ±8.64e15 ms (ECMAScript's TimeClip range, well inside what
// Foundation handles). Never send an arbitrary i64 from a remote journal to
// Foundation as a valid NSDate.
const MAX_DATE_MILLIS: i64 = 8_640_000_000_000_000;

#[derive(Clone, Copy)]
#[repr(usize)]
enum Format {
    JournalTime,
    JournalDay,
    DetailDate,
    UpdatedTime,
    FullDate,
    ShortTime,
    MonthDay,
    MediumDate,
}

/// Equivalent fields to `toLocaleTimeString` with a two-digit hour/minute/second.
pub(crate) fn journal_time(timestamp: i64) -> String {
    format(timestamp, Format::JournalTime)
}

/// Short weekday/month and numeric day, ordered by the user's locale.
pub(crate) fn journal_day(timestamp: i64) -> String {
    format(timestamp, Format::JournalDay)
}

/// A medium date and short time, as used for service state and timer details.
pub(crate) fn detail_date(timestamp: i64) -> String {
    format(timestamp, Format::DetailDate)
}

/// Locale time including seconds, as used by the page's last-updated label.
pub(crate) fn updated_time(timestamp: i64) -> String {
    format(timestamp, Format::UpdatedTime)
}

/// Numeric local date and time including seconds, for journal tooltips.
pub(crate) fn full_date(timestamp: i64) -> String {
    format(timestamp, Format::FullDate)
}

/// A file's modification time at a glance: the time for today, month and day
/// within this year, and a medium date otherwise. Hover shows `detail_date`.
pub(crate) fn file_modified(timestamp: i64, now: i64) -> String {
    use chrono::{DateTime, Datelike, Local};
    let local = |millis| DateTime::from_timestamp_millis(millis).map(|t| t.with_timezone(&Local));
    let (Some(at), Some(now)) = (local(timestamp), local(now)) else {
        return MISSING.into();
    };
    let format = if at.date_naive() == now.date_naive() {
        Format::ShortTime
    } else if at.year() == now.year() {
        Format::MonthDay
    } else {
        Format::MediumDate
    };
    self::format(timestamp, format)
}

/// Identity of the local civil day, including its year, independent of its label.
///
/// The serial day is for grouping only: localized labels omit the year and are
/// therefore not unique. Computing the UTC offset at this instant also handles
/// daylight-saving transitions without assuming every local day is 24 hours.
pub(crate) fn journal_day_key(timestamp: i64) -> Option<String> {
    if valid_timestamp(timestamp) {
        platform::day_key(timestamp)
    } else {
        None
    }
}

fn valid_timestamp(timestamp: i64) -> bool {
    (-MAX_DATE_MILLIS..=MAX_DATE_MILLIS).contains(&timestamp)
}

fn format(timestamp: i64, format: Format) -> String {
    if !valid_timestamp(timestamp) {
        return MISSING.into();
    }
    platform::format(timestamp, format).unwrap_or_else(|| MISSING.into())
}

#[cfg(test)]
mod file_modified_tests {
    use super::file_modified;
    #[test]
    fn age_picks_time_month_day_or_full_date() {
        let now = chrono::Local::now().timestamp_millis();
        let today = file_modified(now, now);
        let earlier_year = file_modified(now - 400 * 86_400_000, now);
        assert!(today.chars().any(|c| c.is_ascii_digit()));
        assert_ne!(today, earlier_year);
        let year = chrono::Datelike::year(&chrono::Local::now()).to_string();
        let previous = (chrono::Datelike::year(&chrono::Local::now()) - 1).to_string();
        assert!(
            !today.contains(&year),
            "today shows a time, not a date: {today}"
        );
        assert!(
            earlier_year.contains(&previous)
                || earlier_year
                    .contains(&(chrono::Datelike::year(&chrono::Local::now()) - 2).to_string()),
            "older files show their year: {earlier_year}"
        );
        assert_eq!(file_modified(i64::MAX, now), "—");
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
mod platform {
    use super::Format;
    use cocoa::{base::nil, foundation::NSString};
    use objc::{
        class,
        declare::ClassDecl,
        msg_send,
        rc::{StrongPtr, autoreleasepool},
        runtime::{Class, Object, Sel},
        sel, sel_impl,
    };
    use std::{
        cell::RefCell,
        ffi::CStr,
        sync::{
            OnceLock,
            atomic::{AtomicU64, Ordering},
        },
    };

    unsafe extern "C" {
        static NSCurrentLocaleDidChangeNotification: *mut Object;
        static NSSystemTimeZoneDidChangeNotification: *mut Object;
    }

    static REVISION: AtomicU64 = AtomicU64::new(0);
    extern "C" fn environment_changed(_: &Object, _: Sel, _: *mut Object) {
        REVISION.fetch_add(1, Ordering::Relaxed);
    }

    /// Foundation notifications may arrive on any thread. They only invalidate
    /// a revision; each thread replaces its own formatters on its next read.
    struct Observer {
        object: StrongPtr,
        center: StrongPtr,
    }

    impl Observer {
        fn new() -> Self {
            static CLASS: OnceLock<&'static Class> = OnceLock::new();
            let class = CLASS.get_or_init(|| {
                let mut class = ClassDecl::new("NewportLocalizedTimeObserver", class!(NSObject))
                    .expect("unique localized-time observer class");
                // SAFETY: The callback has Objective-C's expected notification
                // selector signature, and touches only an atomic revision.
                unsafe {
                    class.add_method(
                        sel!(localizedTimeChanged:),
                        environment_changed as extern "C" fn(&Object, Sel, *mut Object),
                    );
                }
                class.register()
            });
            // SAFETY: `new` transfers one retain; the shared center is retained
            // separately. Both stay alive until removal in Drop.
            unsafe {
                let object = StrongPtr::new(msg_send![*class, new]);
                let center =
                    StrongPtr::retain(msg_send![class!(NSNotificationCenter), defaultCenter]);
                for name in [
                    NSCurrentLocaleDidChangeNotification,
                    NSSystemTimeZoneDidChangeNotification,
                ] {
                    let _: () = msg_send![*center, addObserver:*object
                        selector:sel!(localizedTimeChanged:) name:name object:nil];
                }
                Self { object, center }
            }
        }
    }

    impl Drop for Observer {
        fn drop(&mut self) {
            autoreleasepool(|| {
                // SAFETY: Both pointers remain retained throughout removal.
                unsafe {
                    let _: () = msg_send![*self.center, removeObserver:*self.object];
                }
            });
        }
    }

    struct Cache {
        _observer: Observer,
        revision: u64,
        formatters: Option<Formatters>,
    }

    impl Cache {
        fn new() -> Self {
            Self {
                _observer: Observer::new(),
                revision: 0,
                formatters: None,
            }
        }

        fn get(&mut self) -> &Formatters {
            let revision = REVISION.load(Ordering::Relaxed);
            if self.formatters.is_none() || self.revision != revision {
                // SAFETY: Current locale/time-zone objects are valid for this
                // pool. Formatters retain their own references before it drains.
                self.formatters = Some(unsafe {
                    let locale: *mut Object = msg_send![class!(NSLocale), currentLocale];
                    let zone: *mut Object = msg_send![class!(NSTimeZone), localTimeZone];
                    Formatters::new(locale, zone)
                });
                self.revision = revision;
            }
            self.formatters.as_ref().expect("formatters initialized")
        }
    }

    thread_local! {
        static CACHE: RefCell<Cache> = RefCell::new(Cache::new());
    }

    struct Formatters {
        values: [StrongPtr; 8],
        zone: StrongPtr,
    }

    impl Formatters {
        /// `locale` and `zone` must be valid Foundation instances in this pool.
        unsafe fn new(locale: *mut Object, zone: *mut Object) -> Self {
            // SAFETY: Callers supply NSLocale and NSTimeZone. Each new formatter
            // owns a +1 retain and its locale/zone setters retain or copy inputs.
            unsafe {
                let values = std::array::from_fn(|index| {
                    let formatter = StrongPtr::new(msg_send![class!(NSDateFormatter), new]);
                    let _: () = msg_send![*formatter, setLocale:locale];
                    let _: () = msg_send![*formatter, setTimeZone:zone];
                    match index {
                        0 => {
                            let _: () = msg_send![*formatter, setLocalizedDateFormatFromTemplate:ns_string("jms")];
                            // Foundation's template generator ignores repeated
                            // `j`. Preserve its chosen hour cycle and literals,
                            // but honor the reference UI's two-digit fields.
                            let pattern: *mut Object = msg_send![*formatter, dateFormat];
                            if let Some(pattern) = string(pattern) {
                                let _: () = msg_send![*formatter, setDateFormat:ns_string(&two_digit_time_pattern(&pattern))];
                            }
                        }
                        1 => {
                            let _: () = msg_send![*formatter, setLocalizedDateFormatFromTemplate:ns_string("EEEMMMd")];
                        }
                        2 => {
                            // NSDateFormatterMediumStyle / NSDateFormatterShortStyle.
                            let _: () = msg_send![*formatter, setDateStyle:2_usize];
                            let _: () = msg_send![*formatter, setTimeStyle:1_usize];
                        }
                        3 => {
                            let _: () = msg_send![*formatter, setTimeStyle:2_usize];
                        }
                        4 => {
                            let _: () = msg_send![*formatter, setLocalizedDateFormatFromTemplate:ns_string("yMdjms")];
                        }
                        5 => {
                            let _: () = msg_send![*formatter, setTimeStyle:1_usize];
                        }
                        6 => {
                            let _: () = msg_send![*formatter, setLocalizedDateFormatFromTemplate:ns_string("MMMd")];
                        }
                        _ => {
                            let _: () = msg_send![*formatter, setDateStyle:2_usize];
                        }
                    }
                    formatter
                });
                Self {
                    values,
                    zone: StrongPtr::retain(zone),
                }
            }
        }

        fn format(&self, timestamp: i64, format: Format) -> Option<String> {
            // SAFETY: The NSDate and returned NSString live through the caller's
            // autorelease pool. Copy UTF-8 into Rust before either can expire.
            unsafe {
                let date: *mut Object = msg_send![class!(NSDate), dateWithTimeIntervalSince1970:timestamp as f64 / 1000.];
                let value: *mut Object =
                    msg_send![*self.values[format as usize], stringFromDate:date];
                string(value).filter(|value| !value.is_empty())
            }
        }

        fn day_key(&self, timestamp: i64) -> String {
            // SAFETY: The retained zone accepts a valid NSDate; the offset is
            // queried for this date, not today's DST offset.
            let offset: i64 = unsafe {
                let date: *mut Object = msg_send![class!(NSDate), dateWithTimeIntervalSince1970:timestamp as f64 / 1000.];
                msg_send![*self.zone, secondsFromGMTForDate:date]
            };
            (timestamp.div_euclid(1000) + offset)
                .div_euclid(86_400)
                .to_string()
        }
    }

    /// Return an autoreleased NSString inside the caller's active pool.
    unsafe fn ns_string(value: &str) -> *mut Object {
        // SAFETY: NSString copies the valid UTF-8 bytes supplied by init_str.
        unsafe { StrongPtr::new(NSString::alloc(nil).init_str(value)).autorelease() }
    }

    unsafe fn string(value: *mut Object) -> Option<String> {
        if value.is_null() {
            return None;
        }
        // SAFETY: Callers supply a live NSString. UTF8String is NUL terminated
        // and valid until its owning string is released; copy it immediately.
        unsafe {
            let bytes: *const std::ffi::c_char = msg_send![value, UTF8String];
            (!bytes.is_null()).then(|| CStr::from_ptr(bytes).to_string_lossy().into_owned())
        }
    }

    /// Normalize only unquoted ICU time fields; quoted literals and doubled
    /// apostrophes must survive unchanged in localized patterns.
    fn two_digit_time_pattern(pattern: &str) -> String {
        let mut out = String::with_capacity(pattern.len() + 3);
        let mut quoted = false;
        let mut chars = pattern.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\'' {
                out.push(ch);
                if chars.peek() == Some(&'\'') {
                    out.push(chars.next().expect("peeked apostrophe"));
                } else {
                    quoted = !quoted;
                }
            } else if !quoted && matches!(ch, 'h' | 'H' | 'K' | 'k' | 'm' | 's') {
                while chars.peek() == Some(&ch) {
                    chars.next();
                }
                out.extend([ch, ch]);
            } else {
                out.push(ch);
            }
        }
        out
    }

    pub(super) fn format(timestamp: i64, format: Format) -> Option<String> {
        autoreleasepool(|| CACHE.with_borrow_mut(|cache| cache.get().format(timestamp, format)))
    }

    pub(super) fn day_key(timestamp: i64) -> Option<String> {
        autoreleasepool(|| Some(CACHE.with_borrow_mut(|cache| cache.get().day_key(timestamp))))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn millis(date: &str) -> i64 {
            chrono::DateTime::parse_from_rfc3339(date)
                .unwrap()
                .timestamp_millis()
        }

        fn with_formatters<T>(locale: &str, zone: &str, f: impl FnOnce(&Formatters) -> T) -> T {
            autoreleasepool(|| {
                // Fixed locale and zone keep tests independent of machine
                // preferences, current time, and the test runner's thread.
                unsafe {
                    let locale: *mut Object =
                        msg_send![class!(NSLocale), localeWithLocaleIdentifier:ns_string(locale)];
                    let zone: *mut Object =
                        msg_send![class!(NSTimeZone), timeZoneWithName:ns_string(zone)];
                    assert!(!zone.is_null());
                    f(&Formatters::new(locale, zone))
                }
            })
        }

        fn formatted(locale: &str, zone: &str, at: &str, format: Format) -> String {
            // ICU versions use either normal/narrow/non-breaking spaces before
            // day periods. Assert content and ordering without pinning ICU data.
            with_formatters(locale, zone, |f| f.format(millis(at), format).unwrap())
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        }

        #[test]
        fn honors_locale_hour_cycle_and_pads_journal_fields() {
            let at = "2026-01-02T06:04:05Z";
            assert_eq!(
                formatted("en_US", "UTC", at, Format::JournalTime),
                "06:04:05 AM"
            );
            assert_eq!(
                formatted("en_US@hours=h23", "UTC", at, Format::JournalTime),
                "06:04:05"
            );
            assert_eq!(
                formatted("en_GB", "UTC", at, Format::JournalTime),
                "06:04:05"
            );
            assert_eq!(
                formatted("en_US", "UTC", at, Format::UpdatedTime),
                "6:04:05 AM"
            );
            assert_eq!(
                formatted("ja_JP", "UTC", at, Format::JournalTime),
                "06:04:05"
            );
        }

        #[test]
        fn localizes_day_order_and_detail_fields() {
            let at = "2026-01-02T18:04:05Z";
            assert_eq!(
                formatted("en_US", "UTC", at, Format::JournalDay),
                "Fri, Jan 2"
            );
            assert_eq!(
                formatted("fr_FR", "UTC", at, Format::JournalDay),
                "ven. 2 janv."
            );
            assert_eq!(
                formatted("ja_JP", "UTC", at, Format::JournalDay),
                "1月2日(金)"
            );
            assert_eq!(
                formatted("en_US", "UTC", at, Format::DetailDate),
                "Jan 2, 2026 at 6:04 PM"
            );
            assert_eq!(
                formatted("en_GB", "UTC", at, Format::DetailDate),
                "2 Jan 2026 at 18:04"
            );
            assert_eq!(
                formatted("en_US", "UTC", at, Format::FullDate),
                "1/2/2026, 6:04:05 PM"
            );
            assert_eq!(formatted("en_US", "UTC", at, Format::ShortTime), "6:04 PM");
            assert_eq!(formatted("en_GB", "UTC", at, Format::ShortTime), "18:04");
            assert_eq!(formatted("en_US", "UTC", at, Format::MonthDay), "Jan 2");
            assert_eq!(formatted("en_GB", "UTC", at, Format::MonthDay), "2 Jan");
            assert_eq!(
                formatted("en_US", "UTC", at, Format::MediumDate),
                "Jan 2, 2026"
            );
        }

        #[test]
        fn uses_each_instant_offset_across_dst_transitions() {
            let zone = "America/Los_Angeles";
            assert_eq!(
                formatted("en_US", zone, "2026-03-08T09:59:59Z", Format::JournalTime),
                "01:59:59 AM"
            );
            assert_eq!(
                formatted("en_US", zone, "2026-03-08T10:00:00Z", Format::JournalTime),
                "03:00:00 AM"
            );
            assert_eq!(
                formatted("en_US", zone, "2026-11-01T08:30:00Z", Format::JournalTime),
                "01:30:00 AM"
            );
            assert_eq!(
                formatted("en_US", zone, "2026-11-01T09:30:00Z", Format::JournalTime),
                "01:30:00 AM"
            );
            with_formatters("en_US", zone, |f| {
                assert_eq!(
                    f.day_key(millis("2026-03-08T09:59:59Z")),
                    f.day_key(millis("2026-03-08T10:00:00Z"))
                );
                assert_eq!(
                    f.day_key(millis("2026-11-01T08:30:00Z")),
                    f.day_key(millis("2026-11-01T09:30:00Z"))
                );
                assert_ne!(
                    f.day_key(millis("2026-01-02T07:59:59Z")),
                    f.day_key(millis("2026-01-02T08:00:00Z"))
                );
                assert_ne!(
                    f.day_key(millis("2025-01-02T18:00:00Z")),
                    f.day_key(millis("2031-01-02T18:00:00Z"))
                );
                assert_eq!(
                    f.format(millis("2025-01-02T18:00:00Z"), Format::JournalDay),
                    f.format(millis("2031-01-02T18:00:00Z"), Format::JournalDay)
                );
            });
        }

        #[test]
        fn handles_fractional_offset_zones_and_pre_epoch_midnight() {
            with_formatters("en_GB", "Asia/Kathmandu", |f| {
                let before = millis("2026-01-01T18:14:59Z");
                let after = millis("2026-01-01T18:15:00Z");
                assert_eq!(f.format(before, Format::JournalTime).unwrap(), "23:59:59");
                assert_eq!(f.format(after, Format::JournalTime).unwrap(), "00:00:00");
                assert_ne!(f.day_key(before), f.day_key(after));
            });
            with_formatters("en_GB", "UTC", |f| {
                assert_eq!(f.day_key(-1), "-1");
                assert_eq!(f.day_key(0), "0");
            });
        }

        #[test]
        fn retains_formatters_and_zone_after_creation_pool_drains() {
            let formatters = autoreleasepool(|| unsafe {
                let locale: *mut Object =
                    msg_send![class!(NSLocale), localeWithLocaleIdentifier:ns_string("en_GB")];
                let zone: *mut Object =
                    msg_send![class!(NSTimeZone), timeZoneWithName:ns_string("Asia/Kathmandu")];
                Formatters::new(locale, zone)
            });
            autoreleasepool(|| {
                let timestamp = millis("2026-01-01T18:15:00Z");
                assert_eq!(
                    formatters.format(timestamp, Format::JournalTime).unwrap(),
                    "00:00:00"
                );
                assert!(!formatters.day_key(timestamp).is_empty());
            });
        }

        #[test]
        fn preserves_quoted_time_literals() {
            assert_eq!(two_digit_time_pattern("h:mm:ss a"), "hh:mm:ss a");
            assert_eq!(
                two_digit_time_pattern("H 'h' m 'o''clock' s"),
                "HH 'h' mm 'o''clock' ss"
            );
            assert_eq!(
                two_digit_time_pattern("K 'hours' mm:ss ''"),
                "KK 'hours' mm:ss ''"
            );
        }

        #[test]
        fn refreshes_cached_formatters_after_native_environment_change() {
            autoreleasepool(|| {
                let mut cache = Cache::new();
                // Post the real notifications without changing any preference.
                unsafe {
                    let center: *mut Object =
                        msg_send![class!(NSNotificationCenter), defaultCenter];
                    for name in [
                        NSCurrentLocaleDidChangeNotification,
                        NSSystemTimeZoneDidChangeNotification,
                    ] {
                        let _ = cache.get();
                        let previous = cache.revision;
                        let _: () = msg_send![center, postNotificationName:name object:nil];
                        assert!(REVISION.load(Ordering::Relaxed) > previous);
                        let _ = cache.get();
                        assert!(cache.revision > previous);
                    }
                }
            });
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::Format;
    use chrono::{DateTime, Local};

    pub(super) fn format(timestamp: i64, format: Format) -> Option<String> {
        let date = DateTime::from_timestamp_millis(timestamp)?.with_timezone(&Local);
        let pattern = match format {
            Format::JournalTime | Format::UpdatedTime => "%H:%M:%S",
            Format::JournalDay => "%Y-%m-%d",
            Format::DetailDate => "%Y-%m-%d %H:%M",
            Format::FullDate => "%Y-%m-%d %H:%M:%S",
            Format::ShortTime => "%H:%M",
            Format::MonthDay => "%b %-d",
            Format::MediumDate => "%Y-%m-%d",
        };
        Some(date.format(pattern).to_string())
    }

    pub(super) fn day_key(timestamp: i64) -> Option<String> {
        let date = DateTime::from_timestamp_millis(timestamp)?.with_timezone(&Local);
        Some(
            (date.timestamp() + i64::from(date.offset().local_minus_utc()))
                .div_euclid(86_400)
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_remote_timestamps_are_missing_without_panicking() {
        for timestamp in [
            i64::MIN,
            -MAX_DATE_MILLIS - 1,
            MAX_DATE_MILLIS + 1,
            i64::MAX,
        ] {
            for formatter in [
                journal_time,
                journal_day,
                detail_date,
                updated_time,
                full_date,
            ] {
                assert_eq!(formatter(timestamp), MISSING);
            }
            assert_eq!(journal_day_key(timestamp), None);
        }
    }

    #[test]
    fn epoch_and_negative_timestamps_are_valid_dates() {
        for timestamp in [-1, 0, 1] {
            assert_ne!(journal_time(timestamp), MISSING);
            assert_ne!(journal_day(timestamp), MISSING);
            assert!(journal_day_key(timestamp).is_some());
        }
    }
}
