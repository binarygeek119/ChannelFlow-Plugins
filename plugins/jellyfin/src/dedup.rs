//! Identity is title + year + type. Series use first-air year; episodes
//! inherit the series key and append `SxxEyy`.

/// Fold to lowercase alphanumerics with single spaces, so "Blade Runner: The
/// Final Cut!" and "blade runner the final cut" are the same key.
pub fn normalize(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut last_space = true;
    for c in title.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_alphanumeric() {
            out.push(c);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim().to_string()
}

/// Stable identity for a non-episodic item: `type:title:year`.
pub fn key(media_type: &str, title: &str, year: Option<i32>) -> String {
    format!("{media_type}:{}:{}", normalize(title), year.unwrap_or(0))
}

/// Episodes inherit the series key and append the season/episode numbers.
pub fn episode_key(
    series_title: &str,
    series_year: Option<i32>,
    season: i32,
    episode: i32,
) -> String {
    format!(
        "{}:s{:02}e{:02}",
        key("series", series_title, series_year),
        season,
        episode
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_and_punctuation() {
        assert_eq!(
            normalize("Blade Runner: The Final Cut!"),
            "blade runner the final cut"
        );
        assert_eq!(
            key("movie", "Blade Runner", Some(1982)),
            "movie:blade runner:1982"
        );
    }

    #[test]
    fn episodes_inherit_the_series_key() {
        assert_eq!(
            episode_key("The Expanse", Some(2015), 3, 4),
            "series:the expanse:2015:s03e04"
        );
    }

    #[test]
    fn missing_years_collapse_to_zero() {
        assert_eq!(key("movie", "Untitled", None), "movie:untitled:0");
    }
}