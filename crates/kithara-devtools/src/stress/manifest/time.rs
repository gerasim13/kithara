use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};

use crate::common::timestamp::epoch_days_to_date;

pub(super) fn format_timestamp(time: SystemTime) -> Result<String> {
    let elapsed = time
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?;
    let seconds = elapsed.as_secs();
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let hours = seconds_of_day / 3_600;
    let minutes = (seconds_of_day % 3_600) / 60;
    let seconds = seconds_of_day % 60;
    let (year, month, day) = epoch_days_to_date(days);
    ensure!(
        year <= 9_999,
        "system timestamp year exceeds RFC 3339 range"
    );
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z"
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn formatter_is_stable_at_the_unix_epoch() {
        assert_eq!(
            format_timestamp(UNIX_EPOCH).expect("format epoch"),
            "1970-01-01T00:00:00Z"
        );
        assert_eq!(
            format_timestamp(UNIX_EPOCH + Duration::from_secs(1_704_067_200))
                .expect("format known time"),
            "2024-01-01T00:00:00Z"
        );
    }

    #[test]
    fn formatter_rejects_times_before_the_unix_epoch() {
        let error = format_timestamp(UNIX_EPOCH - Duration::from_secs(1))
            .expect_err("pre-epoch timestamp is not valid evidence");
        assert_eq!(error.to_string(), "system clock is before the Unix epoch");
    }

    #[test]
    fn formatter_preserves_the_rfc3339_year_limit() {
        let last_second = UNIX_EPOCH + Duration::from_secs(253_402_300_799);
        assert_eq!(
            format_timestamp(last_second).expect("last RFC 3339 second"),
            "9999-12-31T23:59:59Z"
        );
        let error = format_timestamp(last_second + Duration::from_secs(1))
            .expect_err("five-digit years are not valid evidence");
        assert_eq!(
            error.to_string(),
            "system timestamp year exceeds RFC 3339 range"
        );
    }
}
