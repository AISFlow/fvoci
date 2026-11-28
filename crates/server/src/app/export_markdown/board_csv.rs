//! A board's cards as a table, `board.csv` of an export of Markdown files: a line a
//! card, by number, with its number, its title and the value of each of its board's
//! properties under the property's name, as the board shows it: an option by its label,
//! a person by the name the reader sees, a file by its name, a related card by its
//! title, a day as `YYYY-MM-DD` and a moment in UTC (`2026-11-28T09:30:00Z`); a span is
//! `from/to`, and several values are joined by `; `.
//!
//! The file is as the web writes the CSV of a view (`logic/csv.ts`): RFC 4180, after a
//! byte order mark, without which a spreadsheet reads Korean as another code page; and a
//! text a spreadsheet would run as a formula (`=`, `+`, `-`, `@`, a tab or a carriage
//! return, also after spaces and invisible marks, or in full width) is written after an
//! apostrophe. A plain number stays a number.

use std::collections::HashMap;

use fvoci_model::{Card, CardPropertyDefinition, PropertyType, is_id, relation_link_card};
use serde_json::Value;

use super::super::export::utc;
use super::names::is_format;
use crate::error::AppError;

/// What the cells call what a card names by identifier: one not here is called by its
/// identifier.
#[derive(Debug, Default)]
pub(super) struct CellNames {
    /// People, by the names the reader sees them by.
    pub(super) users: HashMap<String, String>,
    /// Files, by their names.
    pub(super) files: HashMap<String, String>,
    /// Cards, by their titles.
    pub(super) cards: HashMap<String, String>,
}

/// The table of `cards` under the board's `properties`.
///
/// # Errors
///
/// Returns a server error if the table cannot be written.
pub(super) fn board_csv(
    properties: &[CardPropertyDefinition],
    cards: &[Card],
    names: &CellNames,
) -> Result<Vec<u8>, AppError> {
    let mut writer = ::csv::WriterBuilder::new()
        .terminator(::csv::Terminator::CRLF)
        .from_writer("\u{feff}".as_bytes().to_vec());
    let mut head = vec!["Number".to_owned(), "Title".to_owned()];
    head.extend(properties.iter().map(|property| property.name.clone()));
    let failed =
        |error: ::csv::Error| AppError::internal(format_args!("cannot write board.csv: {error}"));
    writer
        .write_record(head.iter().map(|cell| guarded(cell)))
        .map_err(failed)?;
    for card in cards {
        let mut line = vec![
            card.number.map_or_default(|number| number.to_string()),
            card.title.clone(),
        ];
        line.extend(
            properties
                .iter()
                .map(|property| cell(card, property, names)),
        );
        writer
            .write_record(line.iter().map(|cell| guarded(cell)))
            .map_err(failed)?;
    }
    writer
        .into_inner()
        .map_err(|error| AppError::internal(format_args!("cannot write board.csv: {error}")))
}

/// What the cell of `property` says of `card`.
fn cell(card: &Card, property: &CardPropertyDefinition, names: &CellNames) -> String {
    let value = card.properties.get(&property.id).unwrap_or(&Value::Null);
    let called = |ids: Vec<&str>, of: &HashMap<String, String>| {
        ids.into_iter()
            .map(|id| of.get(id).map_or(id, String::as_str).to_owned())
            .collect::<Vec<_>>()
            .join("; ")
    };
    match property.property_type {
        PropertyType::Select | PropertyType::MultiSelect => {
            let labels: HashMap<&str, &str> = property
                .options
                .iter()
                .map(|option| (option.id.as_str(), option.label.as_str()))
                .collect();
            texts(value)
                .into_iter()
                .map(|id| labels.get(id).copied().unwrap_or(id))
                .collect::<Vec<_>>()
                .join("; ")
        }
        PropertyType::Person | PropertyType::MultiPerson => called(texts(value), &names.users),
        PropertyType::CreatedBy => called(vec![card.created_by.as_str()], &names.users),
        PropertyType::UpdatedBy => called(vec![card.modified_by.as_str()], &names.users),
        PropertyType::File => called(texts(value), &names.files),
        PropertyType::Relation => called(related_cards(value), &names.cards),
        PropertyType::CreatedTime => utc(card.created_at),
        PropertyType::UpdatedTime => utc(card.updated_at),
        PropertyType::Date => date(value),
        PropertyType::Checkbox => (value == &Value::Bool(true)).to_string(),
        PropertyType::Text
        | PropertyType::Number
        | PropertyType::Url
        | PropertyType::Email
        | PropertyType::Phone => match value {
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            _ => String::new(),
        },
    }
}

/// The cards a relation value names: those of its links (`{card, kind, lagDays}`, as
/// they are stored), or of identifiers in their place; what names no card is no card.
pub(super) fn related_cards(value: &Value) -> Vec<&str> {
    let Value::Array(links) = value else {
        return Vec::new();
    };
    links
        .iter()
        .filter_map(|link| relation_link_card(link).or_else(|| link.as_str()))
        .filter(|id| is_id(id))
        .collect()
}

/// The texts of a value: itself, or those of a list.
fn texts(value: &Value) -> Vec<&str> {
    match value {
        Value::String(text) if !text.is_empty() => vec![text.as_str()],
        Value::Array(values) => values.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// A date value, `{from, to?}` of days (`YYYY-MM-DD`) or of moments (milliseconds):
/// `from`, or `from/to`; empty for what is no date.
fn date(value: &Value) -> String {
    let point = |point: Option<&Value>| match point {
        Some(Value::String(day)) => Some(day.clone()),
        Some(Value::Number(at)) => at.as_i64().map(utc),
        _ => None,
    };
    match (point(value.get("from")), point(value.get("to"))) {
        (Some(from), Some(to)) if from != to => format!("{from}/{to}"),
        (Some(from), _) => from,
        (None, _) => String::new(),
    }
}

/// `text`, after an apostrophe where a spreadsheet would run it as a formula.
fn guarded(text: &str) -> String {
    if runs_as_formula(text) {
        format!("'{text}")
    } else {
        text.to_owned()
    }
}

/// Whether a spreadsheet could run `text` as a formula, read as it reads it.
fn runs_as_formula(text: &str) -> bool {
    if text.starts_with(['\t', '\r']) {
        return true;
    }
    // Full-width forms are read as their ASCII ones (`＝` as `=`), as NFKC folds them.
    let head: String = text
        .chars()
        .skip_while(|character| {
            character.is_whitespace() || character.is_control() || is_format(*character)
        })
        .map(|character| match u32::from(character) {
            code @ 0xFF01..=0xFF5E => char::from_u32(code - 0xFEE0).unwrap_or(character),
            _ => character,
        })
        .collect();
    head.starts_with(['=', '+', '-', '@']) && !is_plain_number(&head)
}

/// Whether `text` is a number as a spreadsheet reads one, with its sign.
fn is_plain_number(text: &str) -> bool {
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    !unsigned.is_empty()
        && unsigned.chars().any(|character| character.is_ascii_digit())
        && unsigned.parse::<f64>().is_ok_and(f64::is_finite)
        && unsigned.chars().all(|character| {
            character.is_ascii_digit() || matches!(character, '.' | 'e' | 'E' | '-' | '+')
        })
}

#[cfg(test)]
mod tests {
    use fvoci_model::{Card, CardPropertyDefinition, PropertyOption, PropertyType};
    use serde_json::json;

    use super::{CellNames, board_csv, runs_as_formula};

    #[test]
    fn a_text_a_spreadsheet_would_run_is_guarded_and_a_number_is_not() {
        for text in [
            "=1+1",
            "+82 10",
            "-a",
            "@me",
            " \u{200b}=x",
            "\tx",
            "＝SUM(A1)",
        ] {
            assert!(runs_as_formula(text), "{text:?}");
        }
        for text in ["-5", "+3.5", "1e3", "plain", ""] {
            assert!(!runs_as_formula(text), "{text:?}");
        }
    }

    #[test]
    fn the_cells_say_what_the_board_shows() {
        let mut status = CardPropertyDefinition::new("s", "Status", PropertyType::Select);
        status.options = vec![PropertyOption {
            id: "o1".to_owned(),
            label: "Done".to_owned(),
            ..PropertyOption::default()
        }];
        let properties = [
            status,
            CardPropertyDefinition::new("d", "When", PropertyType::Date),
            CardPropertyDefinition::new("p", "Who", PropertyType::MultiPerson),
            CardPropertyDefinition::new("c", "Checked", PropertyType::Checkbox),
            CardPropertyDefinition::new("t", "Note", PropertyType::Text),
        ];
        let card = Card {
            number: Some(3),
            title: "Plan, \"big\"".to_owned(),
            properties: json!({
                "s": "o1",
                "d": {"from": "2026-12-03", "to": "2026-12-05"},
                "p": ["u1", "u2"],
                "c": true,
                "t": "=HYPERLINK()",
            })
            .as_object()
            .cloned()
            .unwrap_or_default(),
            ..Card::default()
        };
        let names = CellNames {
            users: [("u1".to_owned(), "Kim".to_owned())].into(),
            ..CellNames::default()
        };
        let written = board_csv(&properties, &[card], &names).expect("written");
        let text = String::from_utf8(written).expect("UTF-8");
        assert_eq!(
            text,
            "\u{feff}Number,Title,Status,When,Who,Checked,Note\r\n\
             3,\"Plan, \"\"big\"\"\",Done,2026-12-03/2026-12-05,Kim; u2,true,'=HYPERLINK()\r\n"
        );
    }
}
