use std::fmt;
use std::time::Duration;

use crate::plan::MinFree;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

pub fn parse_duration(input: &str) -> Result<Duration, ParseError> {
    let input = input.trim();
    let invalid = || {
        ParseError(format!(
            "invalid duration {input:?}, expected e.g. 90s, 30m, 24h, 1d or 2w"
        ))
    };
    let (split, unit) = input.char_indices().last().ok_or_else(invalid)?;
    let seconds_per_unit: u64 = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        'w' => 604_800,
        _ => return Err(invalid()),
    };
    let count: u64 = input[..split].parse().map_err(|_| invalid())?;
    let seconds = count.checked_mul(seconds_per_unit).ok_or_else(invalid)?;
    Ok(Duration::from_secs(seconds))
}

pub fn parse_optional_duration(input: &str) -> Result<Option<Duration>, ParseError> {
    if input.trim() == "off" {
        return Ok(None);
    }
    parse_duration(input).map(Some)
}

pub fn parse_min_free(input: &str) -> Result<MinFree, ParseError> {
    let input = input.trim();
    let invalid = || {
        ParseError(format!(
            "invalid size {input:?}, expected e.g. 10%, 50GB or 20GiB"
        ))
    };
    let lowered = input.to_ascii_lowercase();
    if let Some(number) = lowered.strip_suffix('%') {
        let percent: f64 = number.trim().parse().map_err(|_| invalid())?;
        if !(percent > 0.0 && percent <= 100.0) {
            return Err(ParseError(format!(
                "percentage {input:?} must be above 0 and at most 100"
            )));
        }
        return Ok(MinFree::Percent(percent));
    }
    let split = lowered
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(lowered.len());
    let number: f64 = lowered[..split].parse().map_err(|_| invalid())?;
    let multiplier: f64 = match lowered[split..].trim() {
        "" | "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "kib" => 1024.0,
        "mib" => 1024.0_f64.powi(2),
        "gib" => 1024.0_f64.powi(3),
        "tib" => 1024.0_f64.powi(4),
        _ => return Err(invalid()),
    };
    let bytes = number * multiplier;
    if !bytes.is_finite() || bytes >= u64::MAX as f64 {
        return Err(invalid());
    }
    Ok(MinFree::Bytes(bytes as u64))
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in ["KB", "MB", "GB", "TB", "PB", "EB"] {
        if value < 1000.0 {
            break;
        }
        value /= 1000.0;
        unit = next;
    }
    format!("{value:.1} {unit}")
}

pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        86_400.. => format!("{}d", seconds / 86_400),
        3600.. => format!("{}h", seconds / 3600),
        60.. => format!("{}m", seconds / 60),
        _ => format!("{seconds}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        let cases = [
            ("90s", 90),
            ("30m", 1800),
            ("24h", 86_400),
            ("1d", 86_400),
            ("2w", 1_209_600),
            ("0s", 0),
            ("  5m ", 300),
        ];
        for (input, secs) in cases {
            assert_eq!(
                parse_duration(input),
                Ok(Duration::from_secs(secs)),
                "{input}"
            );
        }
    }

    #[test]
    fn invalid_durations() {
        let huge = format!("{}w", u64::MAX);
        for input in [
            "",
            "  ",
            "90",
            "5x",
            "s",
            "-5s",
            "1.5h",
            "1 d",
            "h5",
            "off",
            huge.as_str(),
        ] {
            assert!(parse_duration(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn optional_durations() {
        assert_eq!(parse_optional_duration("off"), Ok(None));
        assert_eq!(parse_optional_duration(" off "), Ok(None));
        assert_eq!(
            parse_optional_duration("2d"),
            Ok(Some(Duration::from_secs(172_800)))
        );
        assert!(parse_optional_duration("never").is_err());
        assert!(parse_optional_duration("").is_err());
    }

    #[test]
    fn min_free_valid() {
        let cases = [
            ("10%", MinFree::Percent(10.0)),
            ("12.5%", MinFree::Percent(12.5)),
            ("100%", MinFree::Percent(100.0)),
            ("512", MinFree::Bytes(512)),
            ("512B", MinFree::Bytes(512)),
            ("50GB", MinFree::Bytes(50_000_000_000)),
            ("50 gb", MinFree::Bytes(50_000_000_000)),
            ("1.5KB", MinFree::Bytes(1500)),
            ("2TB", MinFree::Bytes(2_000_000_000_000)),
            ("10MB", MinFree::Bytes(10_000_000)),
            ("1KiB", MinFree::Bytes(1024)),
            ("20GiB", MinFree::Bytes(20 * 1024 * 1024 * 1024)),
            ("1 mib", MinFree::Bytes(1_048_576)),
            ("1TiB", MinFree::Bytes(1_099_511_627_776)),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_min_free(input), Ok(expected), "{input}");
        }
    }

    #[test]
    fn min_free_invalid() {
        for input in [
            "",
            "%",
            "0%",
            "-5%",
            "101%",
            "abc%",
            "GB",
            "10XB",
            "-5GB",
            ".",
            "1e3",
            "99999999999999999999TB",
            "10 %%",
        ] {
            assert!(parse_min_free(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn bytes_format() {
        let cases = [
            (0, "0 B"),
            (512, "512 B"),
            (999, "999 B"),
            (1000, "1.0 KB"),
            (1500, "1.5 KB"),
            (19_100_000_000, "19.1 GB"),
            (2_500_000_000_000, "2.5 TB"),
        ];
        for (bytes, expected) in cases {
            assert_eq!(format_bytes(bytes), expected);
        }
    }

    #[test]
    fn duration_format() {
        let cases = [
            (0, "0s"),
            (30, "30s"),
            (59, "59s"),
            (60, "1m"),
            (45 * 60 + 59, "45m"),
            (6 * 3600, "6h"),
            (23 * 3600 + 3599, "23h"),
            (21 * 86_400, "21d"),
            (21 * 86_400 + 86_399, "21d"),
        ];
        for (secs, expected) in cases {
            assert_eq!(format_duration(Duration::from_secs(secs)), expected);
        }
    }
}
