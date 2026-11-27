//! What a copy to another workspace keeps of the people, files, references and dates of
//! the blocks it copies: only what the target workspace has, and nothing that names
//! somebody who is not there.

use std::collections::{HashMap, HashSet};

use fvoci_model::{
    Block, BlockType, Body, CardPropertyDefinition, Container, PropertyType, WorkspaceMember,
    add_days, clear_values_of_another_kind, date_property_ids, move_dates,
};
use serde_json::{Value, json};

use super::super::language::Language;
use crate::error::AppError;

pub(super) fn build_member_name_map(members: &[WorkspaceMember]) -> HashMap<String, String> {
    members
        .iter()
        .map(|m| {
            let label = if m.display_name.trim().is_empty() {
                m.username.clone()
            } else {
                m.display_name.clone()
            };
            (m.user_id.clone(), label)
        })
        .collect()
}

#[expect(
    clippy::too_many_arguments,
    reason = "single pass over copied blocks remaps files, references, mentions, dates, and people"
)]
pub(super) fn sanitize_copied_blocks(
    blocks: &mut Vec<Block>,
    container: &Container,
    file_map: &HashMap<String, String>,
    all_copies: &HashMap<String, String>,
    external_titles: &HashMap<String, String>,
    member_names: &HashMap<String, String>,
    present_users: &HashSet<String>,
    days_later: i64,
    language: Language,
    user_id: &str,
    now: i64,
) -> Result<(), AppError> {
    let mut dropped_attachment_ids = HashSet::new();
    blocks.retain_mut(|block| {
        if block.block_type != BlockType::Attachment {
            return true;
        }
        let Some(old_file) = block.attachment_file().map(str::to_owned) else {
            dropped_attachment_ids.insert(block.id.clone());
            return false;
        };
        let Some(new_file) = file_map.get(&old_file) else {
            dropped_attachment_ids.insert(block.id.clone());
            return false;
        };
        block
            .fields
            .insert("fileId".to_owned(), Value::String(new_file.clone()));
        block.fields.remove("attachmentId");
        true
    });

    let date_props = date_property_ids(&container.card_properties);
    let untitled = match language {
        Language::Korean => "제목 없음",
        Language::English => "Untitled",
    };

    for block in blocks.iter_mut() {
        block.drop_malformed_fields();
        if !dropped_attachment_ids.is_empty()
            && let Some(Value::Array(order)) = block.fields.get_mut("contentOrder")
        {
            order.retain(|item| {
                item.as_str()
                    .is_none_or(|id| !dropped_attachment_ids.contains(id))
            });
        }
        if block.block_type == BlockType::Card {
            sanitize_card_properties(
                &mut block.fields,
                &container.card_properties,
                &date_props,
                present_users,
                days_later,
            );
            block.number = None;
        } else if block.block_type == BlockType::Unit && days_later != 0 {
            shift_copied_unit_date(&mut block.fields, days_later);
        } else if block.block_type == BlockType::View {
            sanitize_view_people(&mut block.fields, &container.card_properties, present_users);
        }

        if let Some(body) = block.body.take() {
            block.body = sanitize_copied_body(
                body,
                file_map,
                all_copies,
                external_titles,
                member_names,
                present_users,
                untitled,
            )?;
        }
        user_id.clone_into(&mut block.created_by);
        user_id.clone_into(&mut block.modified_by);
        block.created_at = now;
        block.updated_at = now;
        block.deleted_at = None;
        block.edited_at = 0;
        block.validate().map_err(AppError::from)?;
    }
    Ok(())
}

pub(super) fn sanitize_card_properties(
    fields: &mut fvoci_model::JsonMap,
    definitions: &[CardPropertyDefinition],
    date_props: &HashSet<String>,
    present_users: &HashSet<String>,
    days_later: i64,
) {
    let Some(Value::Object(values)) = fields.get_mut("properties") else {
        return;
    };
    clear_values_of_another_kind(values, definitions);
    if days_later != 0 && !date_props.is_empty() {
        move_dates(values, date_props, days_later);
    }
    for def in definitions {
        match def.property_type {
            PropertyType::CreatedTime
            | PropertyType::UpdatedTime
            | PropertyType::CreatedBy
            | PropertyType::UpdatedBy => {
                values.remove(&def.id);
            }
            PropertyType::Person => {
                let keep = values
                    .get(&def.id)
                    .and_then(Value::as_str)
                    .is_some_and(|uid| present_users.contains(uid));
                if !keep {
                    values.remove(&def.id);
                }
            }
            PropertyType::MultiPerson => {
                if let Some(Value::Array(items)) = values.get_mut(&def.id) {
                    items.retain(|v| v.as_str().is_some_and(|uid| present_users.contains(uid)));
                    if items.is_empty() {
                        values.remove(&def.id);
                    }
                }
            }
            _ => {}
        }
    }
}

pub(super) fn shift_copied_unit_date(fields: &mut fvoci_model::JsonMap, days_later: i64) {
    if let Some(day) = fields
        .get(fvoci_model::UNIT_SCHEDULED_DATE_FIELD)
        .and_then(Value::as_str)
        .and_then(|d| add_days(d, days_later))
    {
        fields.insert(
            fvoci_model::UNIT_SCHEDULED_DATE_FIELD.to_owned(),
            Value::String(day),
        );
    }
}

/// Keeps, of what a copied view says of people, only who is in the target workspace: the
/// values of the person filters at any depth of its filter groups, and, of a view grouped
/// by a person, the groups it shows, hides, folds or counts (the group of nobody stays).
pub(super) fn sanitize_view_people(
    fields: &mut fvoci_model::JsonMap,
    definitions: &[CardPropertyDefinition],
    present_users: &HashSet<String>,
) {
    let person_ids: HashSet<&str> = definitions
        .iter()
        .filter(|d| {
            matches!(
                d.property_type,
                PropertyType::Person
                    | PropertyType::MultiPerson
                    | PropertyType::CreatedBy
                    | PropertyType::UpdatedBy
            )
        })
        .map(|d| d.id.as_str())
        .collect();
    if person_ids.is_empty() {
        return;
    }
    if let Some(Value::Object(filter)) = fields.get_mut("filter") {
        retain_present_people(filter, &person_ids, present_users);
    }
    let grouped_by_person = fields
        .get("groupBy")
        .and_then(Value::as_str)
        .is_some_and(|id| person_ids.contains(id));
    if !grouped_by_person {
        return;
    }
    let kept = |group: &str| group.is_empty() || present_users.contains(group);
    for key in ["visibleGroups", "hiddenGroups", "collapsedGroups"] {
        if let Some(Value::Array(groups)) = fields.get_mut(key) {
            groups.retain(|group| group.as_str().is_some_and(kept));
        }
    }
    if let Some(Value::Object(calculations)) = fields.get_mut("groupCalculations") {
        calculations.retain(|group, _| kept(group));
    }
}

/// Keeps, in the filter group `group` and in each group within it, only the values of its
/// person filters (`person_ids`) that name the viewer (`me`) or somebody `present`.
fn retain_present_people(
    group: &mut fvoci_model::JsonMap,
    person_ids: &HashSet<&str>,
    present: &HashSet<String>,
) {
    let Some(Value::Array(filters)) = group.get_mut("filters") else {
        return;
    };
    for entry in filters.iter_mut().filter_map(Value::as_object_mut) {
        if entry.contains_key("filters") {
            retain_present_people(entry, person_ids, present);
            continue;
        }
        let is_person = entry
            .get("propertyId")
            .and_then(Value::as_str)
            .is_some_and(|id| person_ids.contains(id));
        if is_person && let Some(Value::Array(values)) = entry.get_mut("values") {
            values.retain(|v| {
                v.as_str()
                    .is_some_and(|uid| uid == "me" || present.contains(uid))
            });
        }
    }
}

pub(super) fn sanitize_copied_body(
    body: Body,
    file_map: &HashMap<String, String>,
    all_copies: &HashMap<String, String>,
    external_titles: &HashMap<String, String>,
    member_names: &HashMap<String, String>,
    present_users: &HashSet<String>,
    untitled: &str,
) -> Result<Option<Body>, AppError> {
    let Some(renamed) = body.with_files_renamed(|old| file_map.get(old).cloned()) else {
        return Ok(None);
    };
    let mut doc = renamed.document().clone();
    rewrite_body_nodes(
        &mut doc,
        all_copies,
        external_titles,
        member_names,
        present_users,
        untitled,
    );
    Body::parse(doc).map_err(|error| AppError::from_body_error("body", error))
}

pub(super) fn rewrite_body_nodes(
    node: &mut Value,
    all_copies: &HashMap<String, String>,
    external_titles: &HashMap<String, String>,
    member_names: &HashMap<String, String>,
    present_users: &HashSet<String>,
    untitled: &str,
) {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or("");
    if kind == "reference" {
        let target_id = node
            .get("attrs")
            .and_then(|a| a.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(new_id) = all_copies.get(target_id) {
            if let Some(attrs) = node.get_mut("attrs").and_then(Value::as_object_mut) {
                attrs.insert("id".to_owned(), Value::String(new_id.clone()));
            }
        } else {
            let raw_title = external_titles
                .get(target_id)
                .map_or("", String::as_str)
                .replace('\0', "");
            let label = if raw_title.trim().is_empty() {
                untitled
            } else {
                raw_title.as_str()
            };
            *node = json!({ "type": "text", "text": label });
        }
        return;
    }
    if kind == "mention" {
        let uid = node
            .get("attrs")
            .and_then(|a| a.get("userId"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !present_users.contains(uid) {
            let name = member_names
                .get(uid)
                .map_or("user", String::as_str)
                .replace('\0', "");
            let clean = if name.trim().is_empty() {
                "user"
            } else {
                name.trim()
            };
            *node = json!({ "type": "text", "text": format!("@{clean}") });
        }
        return;
    }
    if let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) {
        for child in children.iter_mut() {
            rewrite_body_nodes(
                child,
                all_copies,
                external_titles,
                member_names,
                present_users,
                untitled,
            );
        }
        coalesce_adjacent_plain_text(children);
    }
}

pub(super) fn coalesce_adjacent_plain_text(children: &mut Vec<Value>) {
    let mut merged: Vec<Value> = Vec::with_capacity(children.len());
    for child in children.drain(..) {
        let plain_text = if child.get("type").and_then(Value::as_str) == Some("text")
            && child.get("marks").is_none()
        {
            child.get("text").and_then(Value::as_str).map(str::to_owned)
        } else {
            None
        };
        if let Some(next_str) = plain_text
            && let Some(prev) = merged.last_mut()
            && prev.get("type").and_then(Value::as_str) == Some("text")
            && prev.get("marks").is_none()
            && let Some(obj) = prev.as_object_mut()
            && let Some(Value::String(existing)) = obj.get_mut("text")
        {
            existing.push_str(&next_str);
            continue;
        }
        merged.push(child);
    }
    *children = merged;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(id: &str) -> CardPropertyDefinition {
        CardPropertyDefinition::new(id.to_owned(), "Owner", PropertyType::Person)
    }

    #[test]
    fn a_copied_view_keeps_only_the_people_of_the_target_workspace() {
        let present: HashSet<String> = HashSet::from(["kept".to_owned()]);
        let mut fields = json!({
            "groupBy": "owner",
            "visibleGroups": ["", "kept", "gone"],
            "hiddenGroups": ["gone"],
            "collapsedGroups": ["kept", "gone"],
            "groupCalculations": {
                "gone": {"calculation": "count", "propertyId": "owner"},
                "": {"calculation": "count", "propertyId": "owner"}
            },
            "filter": {"operation": "and", "filters": [
                {"propertyId": "owner", "condition": "includes", "values": ["me", "gone"]},
                {"operation": "or", "filters": [
                    {"propertyId": "owner", "condition": "includes", "values": ["kept", "gone"]},
                    {"propertyId": "title", "condition": "contains", "values": ["gone"]}
                ]}
            ]}
        });
        let Value::Object(map) = &mut fields else {
            unreachable!("an object");
        };
        sanitize_view_people(map, &[person("owner")], &present);

        assert_eq!(fields["visibleGroups"], json!(["", "kept"]));
        assert_eq!(fields["hiddenGroups"], json!([]));
        assert_eq!(fields["collapsedGroups"], json!(["kept"]));
        assert_eq!(
            fields["groupCalculations"],
            json!({"": {"calculation": "count", "propertyId": "owner"}})
        );
        let filters = &fields["filter"]["filters"];
        assert_eq!(filters[0]["values"], json!(["me"]));
        assert_eq!(filters[1]["filters"][0]["values"], json!(["kept"]));
        // What is no person's filter is left as it is.
        assert_eq!(filters[1]["filters"][1]["values"], json!(["gone"]));
    }

    #[test]
    fn a_view_grouped_by_another_property_keeps_its_groups() {
        let mut fields = json!({"groupBy": "status", "hiddenGroups": ["gone"]});
        let Value::Object(map) = &mut fields else {
            unreachable!("an object");
        };
        sanitize_view_people(map, &[person("owner")], &HashSet::new());
        assert_eq!(fields["hiddenGroups"], json!(["gone"]));
    }
}
