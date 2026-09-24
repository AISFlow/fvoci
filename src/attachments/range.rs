#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedRange {
    Full,
    Bytes { start: u64, end: u64 },
    Invalid,
}

pub fn parse_range(header: Option<&str>, size_bytes: u64) -> ParsedRange {
    let header = match header {
        Some(value) => value.trim(),
        None => return ParsedRange::Full,
    };
    let Some(rest) = header.strip_prefix("bytes=") else {
        return ParsedRange::Invalid;
    };
    let Some((raw_start, raw_end)) = rest.split_once('-') else {
        return ParsedRange::Invalid;
    };
    // Source contract: /^bytes=(\d*)-(\d*)$/ — reject multi-range and non-digits.
    if !raw_start.chars().all(|c| c.is_ascii_digit())
        || !raw_end.chars().all(|c| c.is_ascii_digit())
    {
        return ParsedRange::Invalid;
    }
    if raw_start.is_empty() && raw_end.is_empty() {
        return ParsedRange::Invalid;
    }
    if raw_start.is_empty() {
        let n = raw_end.parse::<u64>().unwrap_or(0);
        if n == 0 || size_bytes == 0 {
            return ParsedRange::Invalid;
        }
        let start = size_bytes.saturating_sub(n);
        return ParsedRange::Bytes {
            start,
            end: size_bytes - 1,
        };
    }
    let start = match raw_start.parse::<u64>() {
        Ok(start) => start,
        Err(_) => return ParsedRange::Invalid,
    };
    if start >= size_bytes {
        return ParsedRange::Invalid;
    }
    if raw_end.is_empty() {
        return ParsedRange::Bytes {
            start,
            end: size_bytes - 1,
        };
    }
    let end = match raw_end.parse::<u64>() {
        Ok(end) => end,
        Err(_) => return ParsedRange::Invalid,
    };
    if end < start {
        return ParsedRange::Invalid;
    }
    ParsedRange::Bytes {
        start,
        end: end.min(size_bytes - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_unit_contract_single_range() {
        assert_eq!(parse_range(None, 100), ParsedRange::Full);
        assert_eq!(
            parse_range(Some("bytes=0-0"), 100),
            ParsedRange::Bytes { start: 0, end: 0 }
        );
        assert_eq!(
            parse_range(Some("bytes=0-999"), 100),
            ParsedRange::Bytes { start: 0, end: 99 }
        );
        assert_eq!(
            parse_range(Some("bytes=99-"), 100),
            ParsedRange::Bytes { start: 99, end: 99 }
        );
        assert_eq!(
            parse_range(Some("bytes=-1"), 100),
            ParsedRange::Bytes { start: 99, end: 99 }
        );
        assert_eq!(
            parse_range(Some("bytes=-200"), 100),
            ParsedRange::Bytes { start: 0, end: 99 }
        );
        assert_eq!(parse_range(Some("bytes=100-"), 100), ParsedRange::Invalid);
        assert_eq!(parse_range(Some("bytes=5-3"), 100), ParsedRange::Invalid);
        assert_eq!(parse_range(Some("bytes=-0"), 100), ParsedRange::Invalid);
        assert_eq!(parse_range(Some("bytes="), 100), ParsedRange::Invalid);
        assert_eq!(
            parse_range(Some("bytes=0-1,5-9"), 100),
            ParsedRange::Invalid
        );
        assert_eq!(parse_range(Some("items=0-1"), 100), ParsedRange::Invalid);
        assert_eq!(parse_range(Some("bytes=0-"), 0), ParsedRange::Invalid);
        assert_eq!(parse_range(Some("bytes=-1"), 0), ParsedRange::Invalid);
        assert_eq!(parse_range(Some("bytes=0-abc"), 100), ParsedRange::Invalid);
        assert_eq!(
            parse_range(Some("bytes=0-1,3-4"), 100),
            ParsedRange::Invalid
        );
    }
}
