use serde::{Deserialize, Deserializer, Serializer};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;

pub fn encode(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

pub fn decode(s: &str) -> Result<Vec<u8>, String> {
    B64.decode(s.trim()).map_err(|e| e.to_string())
}

#[allow(clippy::ptr_arg)]
pub fn serialize<S: Serializer>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&encode(bytes))
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    let s = String::deserialize(deserializer)?;
    decode(&s).map_err(serde::de::Error::custom)
}

pub mod option {
    use super::*;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(b) => serializer.serialize_some(&encode(b)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        let opt = Option::<String>::deserialize(deserializer)?;
        match opt {
            None => Ok(None),
            Some(s) if s.is_empty() => Ok(None),
            Some(s) => decode(&s).map(Some).map_err(serde::de::Error::custom),
        }
    }
}

pub mod vec_of {
    use super::*;
    use serde::ser::SerializeSeq;
    use serde::{Deserialize, Deserializer, Serializer};

    #[allow(clippy::ptr_arg)]
    pub fn serialize<S: Serializer>(
        items: &Vec<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(items.len()))?;
        for item in items {
            seq.serialize_element(&encode(item))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Vec<u8>>, D::Error> {
        let strs = Vec::<String>::deserialize(deserializer)?;
        strs.into_iter()
            .map(|s| decode(&s).map_err(serde::de::Error::custom))
            .collect()
    }
}

/// Decoded size of a standard-base64 payload, ignoring padding, before decode.
pub fn decoded_len_estimate(b64_chars: usize) -> u64 {
    (b64_chars as u64).saturating_mul(3) / 4
}
