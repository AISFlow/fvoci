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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Request;

    // Parent-child frames and the versioned body HTTP API carry these exact strings.
    const FIXTURE: &[u8] = include_bytes!("../fixtures/korean_emoji_mid_edit.v1");
    const FIXTURE_B64: &str = "AQEVC8QVBBUFA+ykkQA=";

    #[test]
    fn encodes_standard_alphabet_with_padding() {
        assert_eq!(encode(&[]), "");
        assert_eq!(encode(&[0xfb]), "+w==");
        assert_eq!(encode(&[0xfb, 0xff]), "+/8=");
        assert_eq!(encode(&[0xfb, 0xff, 0xbf]), "+/+/");
        assert_eq!(encode(FIXTURE), FIXTURE_B64);
    }

    #[test]
    fn decodes_only_canonical_padded_standard() {
        assert_eq!(decode("").unwrap(), Vec::<u8>::new());
        assert_eq!(decode("+w==").unwrap(), [0xfb]);
        assert_eq!(decode(" +/8=\n").unwrap(), [0xfb, 0xff]);
        assert_eq!(decode(FIXTURE_B64).unwrap(), FIXTURE);
        for bad in [
            "+w", "+/8", "+w=", "+w===", "-w==", "_/8=", "+x==", "+/9=", "+w ==", "+w==+w==",
        ] {
            assert!(decode(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn load_frame_bytes_round_trip() {
        let json = format!(
            r#"{{"op":"load","snapshot_b64":"{FIXTURE_B64}","tail_b64":["+w==","+/8="],"encoding":1}}"#
        );
        let request = Request::Load {
            snapshot_b64: Some(FIXTURE.to_vec()),
            tail_b64: vec![vec![0xfb], vec![0xfb, 0xff]],
            encoding: 1,
        };
        assert_eq!(serde_json::to_string(&request).unwrap(), json);
        let Request::Load {
            snapshot_b64,
            tail_b64,
            ..
        } = serde_json::from_str(&json).unwrap()
        else {
            panic!("load frame");
        };
        assert_eq!(snapshot_b64.as_deref(), Some(FIXTURE));
        assert_eq!(tail_b64, [vec![0xfb], vec![0xfb, 0xff]]);
        let Request::Load { snapshot_b64, .. } =
            serde_json::from_str(r#"{"op":"load","snapshot_b64":""}"#).unwrap()
        else {
            panic!("load frame");
        };
        assert_eq!(snapshot_b64, None);
        assert!(serde_json::from_str::<Request>(r#"{"op":"load","tail_b64":["+w"]}"#).is_err());
    }
}
