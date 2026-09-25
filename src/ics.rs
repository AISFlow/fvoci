use chrono::{DateTime, NaiveDate, Utc};

use crate::auth::token::hash_token;

const FOLD_OCTETS: usize = 75;

#[derive(Debug, Clone)]
pub struct IcsTask {
    pub id: String,
    pub title: String,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub due_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

fn ics_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\r', "")
        .replace('\n', "\\n")
}

fn iso_date_stamp(date: NaiveDate) -> String {
    date.format("%Y%m%d").to_string()
}

fn next_iso_date(date: NaiveDate) -> NaiveDate {
    date.succ_opt().unwrap_or(date)
}

fn event_date(task: &IcsTask) -> Option<NaiveDate> {
    if let Some(start) = task.start_date {
        return Some(start);
    }
    if let Some(due) = task.due_date {
        return Some(due);
    }
    task.due_at.map(|due_at| due_at.date_naive())
}

fn event_end(task: &IcsTask, start: NaiveDate) -> NaiveDate {
    if let Some(due) = task.due_date {
        return next_iso_date(due);
    }
    if task.start_date.is_some() {
        if let Some(due_at) = task.due_at {
            return next_iso_date(due_at.date_naive());
        }
    }
    next_iso_date(start)
}

fn fold_ics_line(line: &str) -> String {
    let bytes = line.as_bytes();
    if bytes.len() <= FOLD_OCTETS {
        return line.to_string();
    }
    let mut out = String::new();
    let mut offset = 0;
    let mut first = true;
    while offset < bytes.len() {
        let budget = if first { FOLD_OCTETS } else { FOLD_OCTETS - 1 };
        let mut end = (offset + budget).min(bytes.len());
        while end > offset && !line.is_char_boundary(end) {
            end -= 1;
        }
        if end == offset {
            end = (offset + 1).min(bytes.len());
            while end < bytes.len() && !line.is_char_boundary(end) {
                end += 1;
            }
        }
        if !first {
            out.push_str("\r\n ");
        }
        out.push_str(&line[offset..end]);
        offset = end;
        first = false;
    }
    out
}

fn local_wall_stamp(at: DateTime<Utc>, time_zone: &str) -> Option<String> {
    let offset = zoneinfo_offset_seconds(time_zone, at.timestamp())?;
    let local = at + chrono::Duration::seconds(i64::from(offset));
    Some(local.format("%Y%m%dT%H%M%S").to_string())
}

fn zoneinfo_offset_seconds(time_zone: &str, unix: i64) -> Option<i32> {
    if !iana_name_ok(time_zone) {
        return None;
    }
    let path = std::path::Path::new("/usr/share/zoneinfo").join(time_zone);
    let bytes = std::fs::read(path).ok()?;
    parse_tzif_offset(&bytes, unix)
}

fn iana_name_ok(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 || name.starts_with('/') || name.contains("..") {
        return false;
    }
    let mut parts = 0usize;
    for part in name.split('/') {
        parts += 1;
        if parts > 4
            || part.is_empty()
            || !part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '+' || c == '-')
        {
            return false;
        }
    }
    parts >= 1
}

fn parse_tzif_offset(bytes: &[u8], unix: i64) -> Option<i32> {
    if bytes.len() < 44 || &bytes[0..4] != b"TZif" {
        return None;
    }
    let version = bytes[4];
    let (time_size, header_at) = if version >= b'2' {
        let first = tzif_counts(bytes, 20)?;
        let skip = 44
            + first.timecnt * 5
            + first.typecnt * 6
            + first.charcnt
            + first.leapcnt * 8
            + first.isutcnt
            + first.isstdcnt;
        if bytes.len() < skip + 44 || &bytes[skip..skip + 4] != b"TZif" {
            return None;
        }
        (8usize, skip + 20)
    } else {
        (4usize, 20usize)
    };
    let counts = tzif_counts(bytes, header_at)?;
    let times_at = header_at + 24;
    let types_at = times_at + counts.timecnt * time_size;
    let infos_at = types_at + counts.timecnt;
    if bytes.len() < infos_at + counts.typecnt * 6 {
        return None;
    }
    let mut chosen = 0usize;
    for i in 0..counts.timecnt {
        let start = times_at + i * time_size;
        let t = if time_size == 8 {
            i64::from_be_bytes(bytes[start..start + 8].try_into().ok()?)
        } else {
            i64::from(i32::from_be_bytes(bytes[start..start + 4].try_into().ok()?))
        };
        if t <= unix {
            chosen = usize::from(bytes[types_at + i]);
        } else {
            break;
        }
    }
    if chosen >= counts.typecnt {
        return None;
    }
    let info = infos_at + chosen * 6;
    Some(i32::from_be_bytes(bytes[info..info + 4].try_into().ok()?))
}

struct TzifCounts {
    isutcnt: usize,
    isstdcnt: usize,
    leapcnt: usize,
    timecnt: usize,
    typecnt: usize,
    charcnt: usize,
}

fn tzif_counts(bytes: &[u8], at: usize) -> Option<TzifCounts> {
    if bytes.len() < at + 24 {
        return None;
    }
    Some(TzifCounts {
        isutcnt: u32_at(bytes, at)? as usize,
        isstdcnt: u32_at(bytes, at + 4)? as usize,
        leapcnt: u32_at(bytes, at + 8)? as usize,
        timecnt: u32_at(bytes, at + 12)? as usize,
        typecnt: u32_at(bytes, at + 16)? as usize,
        charcnt: u32_at(bytes, at + 20)? as usize,
    })
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

pub fn stamp_in_zone(at: DateTime<Utc>, time_zone: &str) -> String {
    match local_wall_stamp(at, time_zone) {
        Some(stamp) => stamp,
        None => {
            tracing::warn!(
                tz = %time_zone,
                "ics.stampInZone: invalid timeZone, UTC fallback"
            );
            at.format("%Y%m%dT%H%M%S").to_string()
        }
    }
}

pub fn render_ics(tasks: &[IcsTask], time_zone: &str, now: DateTime<Utc>) -> String {
    let stamp = stamp_in_zone(now, time_zone);
    let tzid = ics_escape(time_zone);
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//FVOCI//ICS//KO".to_string(),
        "CALSCALE:GREGORIAN".to_string(),
        "METHOD:PUBLISH".to_string(),
    ];
    for task in tasks {
        let Some(start) = event_date(task) else {
            continue;
        };
        let end = event_end(task, start);
        lines.push("BEGIN:VEVENT".to_string());
        lines.push(format!("UID:{}@fvoci", task.id));
        lines.push(format!("DTSTAMP;TZID={tzid}:{stamp}"));
        lines.push(format!("DTSTART;VALUE=DATE:{}", iso_date_stamp(start)));
        lines.push(format!("DTEND;VALUE=DATE:{}", iso_date_stamp(end)));
        lines.push(format!("SUMMARY:{}", ics_escape(&task.title)));
        lines.push("END:VEVENT".to_string());
    }
    lines.push("END:VCALENDAR".to_string());
    let mut body = String::new();
    for line in lines {
        body.push_str(&fold_ics_line(&line));
        body.push_str("\r\n");
    }
    body
}

pub fn ics_etag(token_updated_at: DateTime<Utc>, tasks: &[IcsTask]) -> String {
    let mut material = token_updated_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    material.push('|');
    for (i, task) in tasks.iter().enumerate() {
        if i > 0 {
            material.push(',');
        }
        material.push_str(&task.id);
        material.push(':');
        material.push_str(
            &task
                .updated_at
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        );
    }
    format!("\"{}\"", hash_token(&material))
}

pub fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn caldav_multistatus(href: &str, ics: &str, etag: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:response>
    <D:href>{}</D:href>
    <D:propstat>
      <D:prop>
        <D:resourcetype><D:collection/><C:calendar/></D:resourcetype>
        <D:displayname>FVOCI</D:displayname>
        <D:getetag>{}</D:getetag>
        <D:getcontenttype>text/calendar; charset=utf-8</D:getcontenttype>
        <C:supported-calendar-component-set><C:comp name="VEVENT"/></C:supported-calendar-component-set>
        <C:calendar-data>{}</C:calendar-data>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>
"#,
        xml_escape(href),
        xml_escape(etag),
        xml_escape(ics)
    )
}

pub fn caldav_not_found(href: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<D:error xmlns:D="DAV:">
  <D:href>{}</D:href>
  <D:responsedescription>not found</D:responsedescription>
</D:error>
"#,
        xml_escape(href)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use uuid::Uuid;

    fn task(id: &str, title: &str, due: Option<&str>) -> IcsTask {
        IcsTask {
            id: id.to_string(),
            title: title.to_string(),
            start_date: None,
            due_date: due.map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap()),
            due_at: None,
            updated_at: Utc.with_ymd_and_hms(2026, 8, 25, 0, 0, 0).unwrap(),
        }
    }

    #[test]
    fn render_escapes_summary_and_keeps_stable_uid() {
        let ics = render_ics(
            &[task(
                "11111111-1111-1111-1111-111111111111",
                "실험, 1",
                Some("2026-08-26"),
            )],
            "Pacific/Honolulu",
            Utc.with_ymd_and_hms(2026, 8, 25, 15, 0, 0).unwrap(),
        );
        assert!(ics.contains("BEGIN:VCALENDAR"));
        assert!(ics.contains("TZID=Pacific/Honolulu"));
        assert!(ics.contains("SUMMARY:실험\\, 1"));
        assert!(ics.contains("DTSTART;VALUE=DATE:20260826"));
        assert!(ics.contains("UID:11111111-1111-1111-1111-111111111111@fvoci"));
        if std::path::Path::new("/usr/share/zoneinfo/Pacific/Honolulu").exists() {
            assert!(ics.contains("DTSTAMP;TZID=Pacific/Honolulu:20260825T050000"));
        }
    }

    #[test]
    fn cr_and_tzid_are_escaped() {
        let ics = render_ics(
            &[task(
                "33333333-3333-3333-3333-333333333333",
                "줄\r주입",
                Some("2026-08-26"),
            )],
            "Tz;Bad",
            Utc.with_ymd_and_hms(2026, 8, 25, 15, 0, 0).unwrap(),
        );
        assert!(!ics.contains("줄\r"));
        assert!(ics.contains("SUMMARY:줄주입"));
        assert!(ics.contains("TZID=Tz\\;Bad"));
    }

    #[test]
    fn invalid_iana_falls_back_to_utc_stamp() {
        let ics = render_ics(
            &[task(
                "44444444-4444-4444-4444-444444444444",
                "폴백",
                Some("2026-08-26"),
            )],
            "Not/AZone",
            Utc.with_ymd_and_hms(2026, 8, 25, 15, 0, 0).unwrap(),
        );
        assert!(ics.contains("DTSTAMP;TZID=Not/AZone:20260825T150000"));
    }

    #[test]
    fn undated_tasks_are_omitted() {
        let ics = render_ics(
            &[task("22222222-2222-2222-2222-222222222222", "없음", None)],
            "Asia/Seoul",
            Utc::now(),
        );
        assert!(!ics.contains("BEGIN:VEVENT"));
    }

    #[test]
    fn long_summary_is_folded_at_75_octets() {
        let title = "가".repeat(40);
        let ics = render_ics(
            &[task(
                &Uuid::now_v7().to_string(),
                &title,
                Some("2026-08-26"),
            )],
            "Asia/Seoul",
            Utc::now(),
        );
        for raw in ics.split("\r\n") {
            if raw.is_empty() {
                continue;
            }
            assert!(raw.len() <= 75, "line {} octets: {raw}", raw.len());
        }
        assert!(ics.contains("\r\n "));
    }
}
