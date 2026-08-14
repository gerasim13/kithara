use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};

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

const fn epoch_days_to_date(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
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
}
