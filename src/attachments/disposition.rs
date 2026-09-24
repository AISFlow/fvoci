fn percent_encode_utf8(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

pub fn content_disposition_attachment(name: &str) -> String {
    let ascii = name
        .chars()
        .map(|c| {
            if ('\x20'..='\x7e').contains(&c) && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    let encoded = percent_encode_utf8(name);
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}
