//! IMM dates: the third Wednesday of March, June, September and December,
//! used by SOFR, Eurodollar-style and many FX futures. `ImmMonth` is a
//! quarterly contract month with its CME code (H, M, U, Z).

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use super::date::{Date, DateError, Weekday};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ImmError {
    #[error(transparent)]
    Date(#[from] DateError),
    #[error("IMM month must be Mar/Jun/Sep/Dec, got {year:04}-{month:02}")]
    InvalidMonth { year: i32, month: u32 },
    #[error("cannot parse IMM code '{0}' (expected e.g. H27, Z2026)")]
    Parse(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImmMonth {
    year: i32,
    month: u32,
}

impl ImmMonth {
    pub fn new(year: i32, month: u32) -> Result<Self, ImmError> {
        if !matches!(month, 3 | 6 | 9 | 12) {
            return Err(ImmError::InvalidMonth { year, month });
        }
        Ok(Self { year, month })
    }

    pub const fn year(self) -> i32 {
        self.year
    }

    pub const fn month(self) -> u32 {
        self.month
    }

    pub const fn code(self) -> char {
        match self.month {
            3 => 'H',
            6 => 'M',
            9 => 'U',
            _ => 'Z',
        }
    }

    /// Third Wednesday of the month.
    pub fn imm_date(self) -> Result<Date, ImmError> {
        Ok(Date::nth_weekday(3, Weekday::Wednesday, self.year, self.month)?)
    }

    pub fn next(self) -> Self {
        if self.month == 12 { Self { year: self.year + 1, month: 3 } } else { Self { year: self.year, month: self.month + 3 } }
    }

    pub fn prev(self) -> Self {
        if self.month == 3 { Self { year: self.year - 1, month: 12 } } else { Self { year: self.year, month: self.month - 3 } }
    }

    /// First contract month whose IMM date is strictly after `d`.
    pub fn next_after(d: Date) -> Result<Self, ImmError> {
        let (y, m, _) = d.ymd();
        let mut im = Self { year: y, month: m.div_ceil(3) * 3 };
        while im.imm_date()? <= d {
            im = im.next();
        }
        Ok(im)
    }

    /// `n` consecutive contract months starting with the first after `d`.
    pub fn strip(d: Date, n: usize) -> Result<Vec<Self>, ImmError> {
        let mut out = Vec::with_capacity(n);
        let mut im = Self::next_after(d)?;
        for _ in 0..n {
            out.push(im);
            im = im.next();
        }
        Ok(out)
    }
}

impl fmt::Display for ImmMonth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{:02}", self.code(), self.year.rem_euclid(100))
    }
}

/// "H27", "Z26", "M2027". Two-digit years are read as 2000-2099.
impl FromStr for ImmMonth {
    type Err = ImmError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_uppercase();
        let bad = || ImmError::Parse(s.to_owned());
        let mut chars = t.chars();
        let month = match chars.next().ok_or_else(bad)? {
            'H' => 3,
            'M' => 6,
            'U' => 9,
            'Z' => 12,
            _ => return Err(bad()),
        };
        let digits: &str = &t[1..];
        let year: i32 = match digits.len() {
            2 => 2000 + digits.parse::<i32>().map_err(|_| bad())?,
            4 => digits.parse().map_err(|_| bad())?,
            _ => return Err(bad()),
        };
        Self::new(year, month)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type R = Result<(), ImmError>;

    fn d(y: i32, m: u32, dd: u32) -> Date {
        Date::from_ymd(y, m, dd).expect("valid test date")
    }

    #[test]
    fn third_wednesdays() -> R {
        assert_eq!(ImmMonth::new(2026, 12)?.imm_date()?, d(2026, 12, 16));
        assert_eq!(ImmMonth::new(2027, 3)?.imm_date()?, d(2027, 3, 17));
        assert_eq!(ImmMonth::new(2027, 6)?.imm_date()?, d(2027, 6, 16));
        assert_eq!(ImmMonth::new(2027, 9)?.imm_date()?, d(2027, 9, 15));
        Ok(())
    }

    #[test]
    fn next_after_and_strip() -> R {
        assert_eq!(ImmMonth::next_after(d(2026, 10, 13))?, ImmMonth::new(2026, 12)?);
        assert_eq!(ImmMonth::next_after(d(2026, 12, 16))?, ImmMonth::new(2027, 3)?); // on the IMM date itself
        assert_eq!(ImmMonth::next_after(d(2026, 12, 15))?, ImmMonth::new(2026, 12)?);
        let strip = ImmMonth::strip(d(2026, 10, 13), 5)?;
        let s: Vec<String> = strip.iter().map(ToString::to_string).collect();
        assert_eq!(s, ["Z26", "H27", "M27", "U27", "Z27"]);
        assert_eq!(ImmMonth::new(2027, 3)?.prev(), ImmMonth::new(2026, 12)?);
        Ok(())
    }

    #[test]
    fn parse_and_display() -> R {
        assert_eq!("h27".parse::<ImmMonth>()?, ImmMonth::new(2027, 3)?);
        assert_eq!("Z2026".parse::<ImmMonth>()?.to_string(), "Z26");
        assert!("X27".parse::<ImmMonth>().is_err());
        assert!("H7".parse::<ImmMonth>().is_err());
        assert!(ImmMonth::new(2027, 4).is_err());
        Ok(())
    }
}
