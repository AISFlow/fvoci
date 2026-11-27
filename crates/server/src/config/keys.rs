//! That no key of the configuration file is given twice, at the top or inside
//! `file_storage_s3`. The decoded JSON cannot tell: its object keeps one value of a key,
//! the last; so the text is read for its keys.

use std::collections::BTreeSet;
use std::fmt;

use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

use super::NESTED;

/// Checks that every key of the JSON object `text` and of its `file_storage_s3` is given
/// once. The error names the key, never a value.
///
/// # Errors
///
/// Returns an error naming the first key given twice, or one that `text` is no JSON.
pub(super) fn check_each_once(text: &str) -> anyhow::Result<()> {
    let mut reader = serde_json::Deserializer::from_str(text);
    Keys { prefix: None }
        .deserialize(&mut reader)
        .map_err(anyhow::Error::from)
}

/// The keys of an object, checked; a value that is no object passes as it is.
struct Keys {
    /// The key the object is the value of, for the error; `None` at the top.
    prefix: Option<&'static str>,
}

impl<'de> DeserializeSeed<'de> for Keys {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, reader: D) -> Result<(), D::Error> {
        reader.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Keys {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut seen = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                let name = match self.prefix {
                    Some(prefix) => format!("{prefix}.{key}"),
                    None => key,
                };
                return Err(serde::de::Error::custom(format_args!(
                    "{name} is given more than once"
                )));
            }
            if self.prefix.is_none() && key == NESTED {
                map.next_value_seed(Keys {
                    prefix: Some(NESTED),
                })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(())
    }

    fn visit_bool<E>(self, _: bool) -> Result<(), E> {
        Ok(())
    }

    fn visit_i64<E>(self, _: i64) -> Result<(), E> {
        Ok(())
    }

    fn visit_u64<E>(self, _: u64) -> Result<(), E> {
        Ok(())
    }

    fn visit_f64<E>(self, _: f64) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E>(self, _: &str) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<(), E> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_given_twice_is_named_without_its_values() {
        for (text, key) in [
            (
                r#"{"metrics_token": "secret-a", "metrics_token": ""}"#,
                "metrics_token",
            ),
            (r#"{"foo": 1, "foo": 2}"#, "foo"),
            (
                r#"{"file_storage_s3": {"bucket": "secret-a", "bucket": "secret-b"}}"#,
                "file_storage_s3.bucket",
            ),
            (
                r#"{"file_storage_s3": {"secret": "secret-a", "secret": "secret-b"}}"#,
                "file_storage_s3.secret",
            ),
        ] {
            let error = check_each_once(text).expect_err(text).to_string();
            assert!(
                error.starts_with(&format!("{key} is given more than once")),
                "{error}"
            );
            assert!(!error.contains("secret-"), "{error}");
        }
    }

    #[test]
    fn keys_given_once_pass_whatever_their_values() {
        for text in [
            "{}",
            r#"{"a": null, "b": [1, {"c": 1, "c": 2}], "d": {"e": 1, "e": 2}, "f": true}"#,
            r#"{"file_storage_s3": null, "x": 1.5, "y": -3, "z": "text"}"#,
            r#"{"feature_flags": {"flag": "1", "flag": "2"}}"#,
            r#"{"file_storage_s3": {"bucket": "a", "region": "b"}}"#,
            // Keys that differ in case are two keys.
            r#"{"metrics_token": "a", "Metrics_Token": "b"}"#,
            r#"{"file_storage_s3": {"bucket": "a", "Bucket": "b"}}"#,
            // Values of every kind where an object belongs: the error that it is none
            // is the configuration's, and says no value.
            r#"{"file_storage_s3": true}"#,
            r#"{"file_storage_s3": -1}"#,
            r#"{"file_storage_s3": 1.5}"#,
            r#"{"file_storage_s3": 18446744073709551616}"#,
            r#"{"file_storage_s3": "x\nyé"}"#,
            r#"{"file_storage_s3": [1, {"a": 1, "a": 2}]}"#,
        ] {
            check_each_once(text).expect(text);
        }
    }
}
