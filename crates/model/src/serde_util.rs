//! Serde helpers shared by the wire types.

use serde::{Deserialize, Deserializer};

/// `skip_serializing_if` predicate for integer members left out of the JSON while zero.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes the field by reference"
)]
pub(crate) fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// Deserializes a present field as `Some(T)`, refusing explicit JSON `null`.
pub(crate) fn non_null<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Deserializes a present deletion timestamp as `Some(i64)` strictly greater than zero,
/// refusing `null`, `0`, and negative timestamps.
pub(crate) fn positive_millis<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let millis = i64::deserialize(deserializer)?;
    if millis > 0 {
        Ok(Some(millis))
    } else {
        Err(serde::de::Error::custom(
            "a time of deletion is after the epoch",
        ))
    }
}

/// Deserializes a patch member where omission is `None`, explicit `null` is `Some(None)`
/// (clearing the field), and a value is `Some(Some(value))`.
#[expect(
    clippy::option_option,
    reason = "tri-state patch helper returns None when omitted, Some(None) on null, and Some(Some(v)) on value"
)]
pub(crate) fn nullable_patch<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}
