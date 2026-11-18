//! Small helpers shared by the commands: time formatting and HTML for mentions.

use matrix_sdk::ruma::{MilliSecondsSinceUnixEpoch, UserId};

/// Formats a Matrix timestamp as UTC `YYYY-MM-DD HH:MM:SSZ`.
pub fn format_ts(ts: MilliSecondsSinceUnixEpoch) -> String {
    format_ms(u64::from(ts.0))
}

/// Formats milliseconds since the Unix epoch as UTC `YYYY-MM-DD HH:MM:SSZ`.
pub fn format_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to (year, month, day).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The first `max` characters of `text` on one line, for previews.
pub fn preview(text: &str, max: usize) -> String {
    text.chars()
        .take(max)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// The HTML body for `text` with the first occurrence of `mention` turned into
/// a `matrix.to` link to `user_id`, or `None` when `mention` does not occur.
pub fn mention_html(text: &str, mention: &str, user_id: &UserId) -> Option<String> {
    let start = text.find(mention)?;
    let end = start + mention.len();
    Some(format!(
        "{}<a href=\"https://matrix.to/#/{}\">{}</a>{}",
        escape_html(&text[..start]),
        escape_html(user_id.as_str()),
        escape_html(mention),
        escape_html(&text[end..]),
    ))
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::user_id;

    use super::*;

    #[test]
    fn mention_html_links_only_the_first_occurrence_and_escapes_the_rest() {
        let html = mention_html("<b> Alice & Alice", "Alice", user_id!("@alice:example.org"));
        assert_eq!(
            html.as_deref(),
            Some(
                "&lt;b&gt; <a href=\"https://matrix.to/#/@alice:example.org\">Alice</a> &amp; Alice"
            )
        );
    }

    #[test]
    fn mention_html_is_none_when_the_text_is_missing() {
        assert_eq!(
            mention_html("hello", "Bob", user_id!("@bob:example.org")),
            None
        );
    }

    #[test]
    fn format_ms_handles_leap_days_and_epoch() {
        assert_eq!(format_ms(0), "1970-01-01 00:00:00Z");
        // 2024-02-29 12:34:56 UTC
        assert_eq!(format_ms(1_709_210_096_000), "2024-02-29 12:34:56Z");
    }
}
