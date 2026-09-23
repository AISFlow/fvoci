use serde_json::Value;

use crate::db::identity::{FamilyNamePatch, ProfilePatch};
use crate::error::{AppError, ProblemCode};

const PATCH_ME_FIELDS: &[&str] = &[
    "givenName",
    "familyName",
    "locale",
    "timezone",
    "weekStartsOn",
    "textScale",
];

pub fn parse_patch_me(value: Value) -> Result<ProfilePatch, AppError> {
    let obj = value
        .as_object()
        .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/"))?;

    for key in obj.keys() {
        if !PATCH_ME_FIELDS.contains(&key.as_str()) {
            return Err(AppError::with_source(
                ProblemCode::InvalidInput,
                format!("/{}", key),
            ));
        }
    }

    let given_name = match obj.get("givenName") {
        None => {
            return Err(AppError::with_source(
                ProblemCode::InvalidInput,
                "/givenName",
            ));
        }
        Some(v) if v.is_null() => {
            return Err(AppError::with_source(
                ProblemCode::InvalidInput,
                "/givenName",
            ));
        }
        Some(v) => v
            .as_str()
            .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/givenName"))?
            .to_string(),
    };

    let family_name = match obj.get("familyName") {
        None => FamilyNamePatch::Preserve,
        Some(v) if v.is_null() => FamilyNamePatch::Clear,
        Some(v) => {
            let raw = v
                .as_str()
                .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/familyName"))?;
            FamilyNamePatch::Set(raw.to_string())
        }
    };

    let locale = parse_optional_string(obj, "locale")?;
    let timezone = parse_optional_string(obj, "timezone")?;
    let week_starts_on = parse_optional_i32(obj, "weekStartsOn")?;
    let text_scale = parse_optional_i16(obj, "textScale")?;

    Ok(ProfilePatch {
        given_name,
        family_name,
        locale,
        timezone,
        week_starts_on,
        text_scale,
    })
}

fn parse_optional_string(
    obj: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<String>, AppError> {
    match obj.get(field) {
        None => Ok(None),
        Some(v) if v.is_null() => Err(AppError::with_source(
            ProblemCode::InvalidInput,
            format!("/{}", field),
        )),
        Some(v) => Ok(Some(
            v.as_str()
                .ok_or_else(|| {
                    AppError::with_source(ProblemCode::InvalidInput, format!("/{}", field))
                })?
                .to_string(),
        )),
    }
}

fn parse_optional_i32(
    obj: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<i32>, AppError> {
    match obj.get(field) {
        None => Ok(None),
        Some(v) if v.is_null() => Err(AppError::with_source(
            ProblemCode::InvalidInput,
            format!("/{}", field),
        )),
        Some(v) => Ok(Some(
            v.as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| {
                    AppError::with_source(ProblemCode::InvalidInput, format!("/{}", field))
                })?,
        )),
    }
}

fn parse_optional_i16(
    obj: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<i16>, AppError> {
    match obj.get(field) {
        None => Ok(None),
        Some(v) if v.is_null() => Err(AppError::with_source(
            ProblemCode::InvalidInput,
            format!("/{}", field),
        )),
        Some(v) => Ok(Some(
            v.as_i64()
                .and_then(|n| i16::try_from(n).ok())
                .ok_or_else(|| {
                    AppError::with_source(ProblemCode::InvalidInput, format!("/{}", field))
                })?,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::identity::FamilyNamePatch;
    use serde_json::json;

    #[test]
    fn patch_me_family_name_omitted_preserves() {
        let patch = parse_patch_me(json!({"givenName": "Renamed"})).expect("parse patch");
        assert_eq!(patch.given_name, "Renamed");
        assert!(matches!(patch.family_name, FamilyNamePatch::Preserve));
    }

    #[test]
    fn patch_me_family_name_null_clears() {
        let patch = parse_patch_me(json!({"givenName": "Renamed", "familyName": null}))
            .expect("parse patch");
        assert_eq!(patch.given_name, "Renamed");
        assert!(matches!(patch.family_name, FamilyNamePatch::Clear));
    }

    #[test]
    fn patch_me_family_name_string_sets_value() {
        let patch = parse_patch_me(json!({"givenName": "Renamed", "familyName": "Kim"}))
            .expect("parse patch");
        assert_eq!(patch.given_name, "Renamed");
        assert!(matches!(patch.family_name, FamilyNamePatch::Set(ref name) if name == "Kim"));
    }
}
