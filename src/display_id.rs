use regex::Regex;
use std::sync::LazyLock;

static DISPLAY_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z0-9-]{2,32})-(\d{1,9})$").expect("display id regex"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDisplayId {
    pub prefix: String,
    pub number: i32,
}

pub fn parse_display_id(raw: &str) -> Option<ParsedDisplayId> {
    let trimmed = raw.trim();
    let caps = DISPLAY_ID_RE.captures(trimmed)?;
    let prefix = caps.get(1)?.as_str().to_uppercase();
    let digits = caps.get(2)?.as_str();
    let number = digits.parse().ok()?;
    Some(ParsedDisplayId { prefix, number })
}

pub fn format_display_id(prefix: &str, number: i32) -> String {
    format!("{prefix}-{number}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_and_wiki_display_ids() {
        assert_eq!(
            parse_display_id("lab-12"),
            Some(ParsedDisplayId {
                prefix: "LAB".to_string(),
                number: 12
            })
        );
        assert_eq!(
            parse_display_id(" WIKI-3 "),
            Some(ParsedDisplayId {
                prefix: "WIKI".to_string(),
                number: 3
            })
        );
        assert!(parse_display_id("x").is_none());
    }
}
