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
    let rest = header.strip_prefix("bytes=").unwrap_or("");
    let Some((raw_start, raw_end)) = rest.split_once('-') else {
        return ParsedRange::Invalid;
    };
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
    let start = raw_start.parse::<u64>().unwrap_or(size_bytes);
    if start >= size_bytes {
        return ParsedRange::Invalid;
    }
    if raw_end.is_empty() {
        return ParsedRange::Bytes {
            start,
            end: size_bytes - 1,
        };
    }
    let end = raw_end.parse::<u64>().unwrap_or(0);
    if end < start {
        return ParsedRange::Invalid;
    }
    ParsedRange::Bytes {
        start,
        end: end.min(size_bytes - 1),
    }
}
