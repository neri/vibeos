//! 日付と時刻の計算
//!
//! 時刻は「1970-01-01 00:00:00 からの秒数」で持つ（タイムゾーンは扱わない。PC の時計は
//! ふつう現地時刻なので、その値をそのまま使う）。

#![cfg_attr(not(test), no_std)]

use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// 1970-01-01 からの日数（グレゴリオ暦。Howard Hinnant の方法）
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

pub fn is_leap(year: u16) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

pub fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl DateTime {
    pub const EPOCH: DateTime = DateTime { year: 1970, month: 1, day: 1, hour: 0, minute: 0, second: 0 };

    /// 範囲外の値なら None
    pub fn new(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Option<DateTime> {
        let ok = (1970..=9999).contains(&year)
            && (1..=12).contains(&month)
            && day >= 1
            && day <= days_in_month(year, month)
            && hour < 24
            && minute < 60
            && second < 60;
        ok.then_some(DateTime { year, month, day, hour, minute, second })
    }

    pub fn from_unix(secs: u64) -> DateTime {
        let days = (secs / 86_400) as i64;
        let rem = secs % 86_400;
        let (y, m, d) = civil_from_days(days);
        DateTime {
            year: y as u16,
            month: m as u8,
            day: d as u8,
            hour: (rem / 3600) as u8,
            minute: (rem / 60 % 60) as u8,
            second: (rem % 60) as u8,
        }
    }

    pub fn to_unix(&self) -> u64 {
        let days = days_from_civil(self.year as i64, self.month as u32, self.day as u32);
        days as u64 * 86_400 + self.hour as u64 * 3600 + self.minute as u64 * 60 + self.second as u64
    }

    /// 0 = 日曜
    pub fn weekday(&self) -> u8 {
        let days = days_from_civil(self.year as i64, self.month as u32, self.day as u32);
        (days + 4).rem_euclid(7) as u8 // 1970-01-01 は木曜
    }

    /// FAT の日付（年は 1980〜2107 に収める）
    pub fn fat_date(&self) -> u16 {
        let y = self.year.clamp(1980, 2107) - 1980;
        (y << 9) | (self.month as u16) << 5 | self.day as u16
    }

    /// FAT の時刻（秒は 2 秒単位）
    pub fn fat_time(&self) -> u16 {
        (self.hour as u16) << 11 | (self.minute as u16) << 5 | (self.second as u16 / 2)
    }

    /// FAT の日付と時刻から。壊れた値なら None
    pub fn from_fat(date: u16, time: u16) -> Option<DateTime> {
        DateTime::new(
            1980 + (date >> 9),
            ((date >> 5) & 0xF) as u8,
            (date & 0x1F) as u8,
            (time >> 11) as u8,
            ((time >> 5) & 0x3F) as u8,
            ((time & 0x1F) * 2) as u8,
        )
    }
}

impl fmt::Display for DateTime {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_unix_times() {
        assert_eq!(DateTime::EPOCH.to_unix(), 0);
        let t = DateTime::new(2000, 2, 29, 12, 34, 56).unwrap();
        assert_eq!(t.to_unix(), 951_827_696);
        assert_eq!(DateTime::from_unix(951_827_696), t);
        let t = DateTime::new(2026, 10, 11, 21, 15, 0).unwrap();
        assert_eq!(DateTime::from_unix(t.to_unix()), t);
        assert_eq!(DateTime::from_unix(4_102_444_799).to_string(), "2099-12-31 23:59:59");
    }

    #[test]
    fn round_trip_every_day_for_a_century() {
        let mut secs = 0u64;
        let mut prev = DateTime::EPOCH;
        while secs < 4_102_444_800 {
            let t = DateTime::from_unix(secs);
            assert_eq!(t.to_unix(), secs);
            assert!(t > prev || secs == 0);
            assert!(DateTime::new(t.year, t.month, t.day, t.hour, t.minute, t.second).is_some());
            prev = t;
            secs += 86_400 + 3_601; // 毎日少しずつ時刻もずらす
        }
    }

    #[test]
    fn validation_and_leap_years() {
        assert!(DateTime::new(2024, 2, 29, 0, 0, 0).is_some());
        assert!(DateTime::new(2025, 2, 29, 0, 0, 0).is_none());
        assert!(DateTime::new(1900, 1, 1, 0, 0, 0).is_none());
        assert!(DateTime::new(2000, 13, 1, 0, 0, 0).is_none());
        assert!(DateTime::new(2000, 4, 31, 0, 0, 0).is_none());
        assert!(DateTime::new(2000, 1, 1, 24, 0, 0).is_none());
        assert!(is_leap(2000) && !is_leap(1900) && is_leap(2024));
    }

    #[test]
    fn weekdays() {
        assert_eq!(DateTime::EPOCH.weekday(), 4); // 木曜
        assert_eq!(DateTime::new(2026, 10, 11, 0, 0, 0).unwrap().weekday(), 0); // 日曜
    }

    #[test]
    fn fat_format() {
        let t = DateTime::new(2026, 10, 11, 21, 15, 37).unwrap();
        let (d, tm) = (t.fat_date(), t.fat_time());
        assert_eq!(d, (46 << 9) | (10 << 5) | 11);
        // FAT は秒が 2 秒単位なので 37 → 36
        assert_eq!(DateTime::from_fat(d, tm), DateTime::new(2026, 10, 11, 21, 15, 36));
        assert_eq!(DateTime::from_fat(0, 0), None); // 月 0 は不正
    }
}
