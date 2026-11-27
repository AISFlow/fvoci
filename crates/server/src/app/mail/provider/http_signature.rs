//! OCI's request signature: draft-cavage HTTP Signatures, RSA-SHA256 over the named
//! headers, under the key `tenancy/user/fingerprint`.
//! <https://docs.oracle.com/en-us/iaas/Content/API/Concepts/signingrequests.htm>

use anyhow::{Context as _, bail};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// The API signing key of an OCI user. Its parts are never shown.
pub(super) struct SigningKey(RsaKeyPair);

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SigningKey(<hidden>)")
    }
}

impl SigningKey {
    /// The key of a PEM file as the OCI console and `openssl` write it: PKCS#8
    /// (`BEGIN PRIVATE KEY`) or PKCS#1 (`BEGIN RSA PRIVATE KEY`), unencrypted, of 2048
    /// to 8192 bits.
    ///
    /// # Errors
    /// Returns an error that says what is wrong with the file, never its contents.
    pub(super) fn from_pem(pem: &str) -> anyhow::Result<Self> {
        let (label, der) = pem_block(pem)?;
        let pair = match label {
            "PRIVATE KEY" => RsaKeyPair::from_pkcs8(&der),
            "RSA PRIVATE KEY" => RsaKeyPair::from_der(&der),
            "ENCRYPTED PRIVATE KEY" => bail!("the key is encrypted; give it without a passphrase"),
            _ => bail!("the file holds no RSA private key"),
        };
        pair.map(Self)
            .map_err(|rejected| anyhow::anyhow!("the RSA key is not usable: {rejected}"))
    }

    /// The signature of `message`, in base64.
    fn sign(&self, message: &[u8]) -> String {
        let mut signature = vec![0; self.0.public_modulus_len()];
        self.0
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                message,
                &mut signature,
            )
            .expect("a buffer of the modulus' length takes an RSA signature");
        STANDARD.encode(signature)
    }
}

/// The label and the bytes of the first PEM block of `pem`.
fn pem_block(pem: &str) -> anyhow::Result<(&str, Vec<u8>)> {
    let begin = pem
        .find("-----BEGIN ")
        .context("the file holds no PEM block")?;
    let rest = &pem[begin + "-----BEGIN ".len()..];
    let label_end = rest.find("-----").context("the PEM block has no label")?;
    let label = &rest[..label_end];
    let body = &rest[label_end + "-----".len()..];
    let end = body.find("-----END ").context("the PEM block has no end")?;
    let text: String = body[..end]
        .lines()
        // A key of the older OpenSSL form names its encryption in headers.
        .filter(|line| !line.contains(':'))
        .flat_map(|line| line.trim().chars())
        .collect();
    if body[..end].contains("ENCRYPTED") {
        bail!("the key is encrypted; give it without a passphrase");
    }
    let der = STANDARD
        .decode(text)
        .ok()
        .context("the PEM block is not base64")?;
    Ok((label, der))
}

/// The `Authorization` header of a request whose `headers`, in this order, are signed
/// under `key_id`: each as `name: value` on a line, the pseudo-header
/// `(request-target)` as `method path`, names in lower case.
pub(super) fn authorization(key: &SigningKey, key_id: &str, headers: &[(&str, &str)]) -> String {
    let names = headers
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(" ");
    let signature = key.sign(signing_string(headers).as_bytes());
    format!(
        "Signature version=\"1\",keyId=\"{key_id}\",algorithm=\"rsa-sha256\",\
         headers=\"{names}\",signature=\"{signature}\""
    )
}

/// The lines that are signed.
fn signing_string(headers: &[(&str, &str)]) -> String {
    headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The example key and requests of OCI's documentation on signing requests, with the
/// signatures it gives for them: the page as it was on 2019-12-10, which printed them
/// (today's page leaves them out), read 2026-11-14:
/// <https://web.archive.org/web/20191210134508/https://docs.cloud.oracle.com/iaas/Content/API/Concepts/signingrequests.htm>.
/// The example key is of 1024 bits, which aws-lc-rs signs with no longer; PKCS#1 v1.5
/// signatures are deterministic, so a published signature that verifies over the
/// signing string made here is the signature this signer makes with that key.
#[cfg(test)]
mod tests {
    use aws_lc_rs::signature::{RSA_PKCS1_1024_8192_SHA256_FOR_LEGACY_USE_ONLY, UnparsedPublicKey};

    use super::*;

    const EXAMPLE_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDCFENGw33yGihy92pDjZQhl0C3
6rPJj+CvfSC8+q28hxA161QFNUd13wuCTUcq0Qd2qsBe/2hFyc2DCJJg0h1L78+6
Z4UMR7EOcpfdUE9Hf3m/hs+FUR45uBJeDK1HSFHD8bHKD6kv8FPGfJTotc+2xjJw
oYi+1hqp1fIekaxsyQIDAQAB
-----END PUBLIC KEY-----";
    const EXAMPLE_PRIVATE_KEY: &str = "-----BEGIN RSA PRIVATE KEY-----
MIICXgIBAAKBgQDCFENGw33yGihy92pDjZQhl0C36rPJj+CvfSC8+q28hxA161QF
NUd13wuCTUcq0Qd2qsBe/2hFyc2DCJJg0h1L78+6Z4UMR7EOcpfdUE9Hf3m/hs+F
UR45uBJeDK1HSFHD8bHKD6kv8FPGfJTotc+2xjJwoYi+1hqp1fIekaxsyQIDAQAB
AoGBAJR8ZkCUvx5kzv+utdl7T5MnordT1TvoXXJGXK7ZZ+UuvMNUCdN2QPc4sBiA
QWvLw1cSKt5DsKZ8UETpYPy8pPYnnDEz2dDYiaew9+xEpubyeW2oH4Zx71wqBtOK
kqwrXa/pzdpiucRRjk6vE6YY7EBBs/g7uanVpGibOVAEsqH1AkEA7DkjVH28WDUg
f1nqvfn2Kj6CT7nIcE3jGJsZZ7zlZmBmHFDONMLUrXR/Zm3pR5m0tCmBqa5RK95u
412jt1dPIwJBANJT3v8pnkth48bQo/fKel6uEYyboRtA5/uHuHkZ6FQF7OUkGogc
mSJluOdc5t6hI1VsLn0QZEjQZMEOWr+wKSMCQQCC4kXJEsHAve77oP6HtG/IiEn7
kpyUXRNvFsDE0czpJJBvL/aRFUJxuRK91jhjC68sA7NsKMGg5OXb5I5Jj36xAkEA
gIT7aFOYBFwGgQAQkWNKLvySgKbAZRTeLBacpHMuQdl1DfdntvAyqpAZ0lY0RKmW
G6aFKaqQfOXKCyWoUiVknQJAXrlgySFci/2ueKlIE1QqIiLSZ8V8OlpFLRnb1pzI
7U1yQXnTAEFYM560yJlzUpOb1V4cScGd365tiSMvxLOvTA==
-----END RSA PRIVATE KEY-----";
    const DATE: &str = "Thu, 05 Jan 2014 21:31:40 GMT";
    const HOST: &str = "iaas.us-phoenix-1.oraclecloud.com";

    /// The published POST: its body, of 316 bytes, whose hash the page prints.
    const POST_BODY: &str = "{\n    \"compartmentId\": \"ocid1.compartment.oc1..aaaaaaaam3we6vgnherjq5q2idnccdflvjsnog7mlr6rtdb25gilchfeyjxa\",\n    \"instanceId\": \"ocid1.instance.oc1.phx.abuw4ljrlsfiqw6vzzxb43vyypt4pkodawglp3wqxjqofakrwvou52gb6s5a\",\n    \"volumeId\": \"ocid1.volume.oc1.phx.abyhqljrgvttnlx73nmrwfaux7kcvzfs3s66izvxf2h4lgvyndsdsnoiwr5q\"\n}";

    fn verify(headers: &[(&str, &str)], published: &str) {
        let (_, public_key) = pem_block(EXAMPLE_PUBLIC_KEY).expect("the public key");
        let signature = STANDARD.decode(published).expect("base64");
        UnparsedPublicKey::new(&RSA_PKCS1_1024_8192_SHA256_FOR_LEGACY_USE_ONLY, public_key)
            .verify(signing_string(headers).as_bytes(), &signature)
            .expect("the published signature is the one of this signing string");
    }

    #[test]
    fn the_published_get_is_signed_over_these_lines() {
        verify(
            &[
                ("date", DATE),
                (
                    "(request-target)",
                    "get /20160918/instances?availabilityDomain=Pjwf%3A%20PHX-AD-1&\
                     compartmentId=ocid1.compartment.oc1..aaaaaaaam3we6vgnherjq5q2idnccdflvjsnog7mlr6rtdb25gilchfeyjxa&\
                     displayName=TeamXInstances&\
                     volumeId=ocid1.volume.oc1.phx.abyhqljrgvttnlx73nmrwfaux7kcvzfs3s66izvxf2h4lgvyndsdsnoiwr5q",
                ),
                ("host", HOST),
            ],
            "GBas7grhyrhSKHP6AVIj/h5/Vp8bd/peM79H9Wv8kjoaCivujVXlpbKLjMPeDUhxkFIWtTtLBj3sUzaFj34XE6YZAHc9r2DmE4pMwOAy/kiITcZxa1oHPOeRheC0jP2dqbTll8fmTZVwKZOKHYPtrLJIJQHJjNvxFWeHQjMaR7M=",
        );
    }

    #[test]
    fn the_published_post_is_signed_over_its_body_hash_and_length() {
        use sha2::{Digest as _, Sha256};
        assert_eq!(POST_BODY.len(), 316);
        let hash = STANDARD.encode(Sha256::digest(POST_BODY.as_bytes()));
        assert_eq!(hash, "V9Z20UJTvkvpJ50flBzKE32+6m2zJjweHpDMX/U4Uy0=");
        verify(
            &[
                ("date", DATE),
                ("(request-target)", "post /20160918/volumeAttachments"),
                ("host", HOST),
                ("content-length", "316"),
                ("content-type", "application/json"),
                ("x-content-sha256", &hash),
            ],
            "Mje8vIDPlwIHmD/cTDwRxE7HaAvBg16JnVcsuqaNRim23fFPgQfLoOOxae6WqKb1uPjYEl0qIdazWaBy/Ml8DRhqlocMwoSXv0fbukP8J5N80LCmzT/FFBvIvTB91XuXI3hYfP9Zt1l7S6ieVadHUfqBedWH0itrtPJBgKmrWso=",
        );
    }

    #[test]
    fn the_example_key_of_1024_bits_is_refused() {
        let refused = SigningKey::from_pem(EXAMPLE_PRIVATE_KEY).expect_err("too small");
        assert!(refused.to_string().contains("not usable"), "{refused}");
    }

    /// A key of 2048 bits made with OpenSSL 3.5 for this test, in both of its PEM forms,
    /// and the signature `openssl dgst -sha256 -sign` makes of the published POST's
    /// lines with it: an implementation apart from aws-lc-rs signs the same.
    #[test]
    fn a_key_of_either_form_signs_as_openssl_does() {
        const OPENSSL: &str = "PsTveqP+NasscCJoRTwbeXznjRTk4QdNmOZRFowdMukhh3ojzcrLSDhBFcYZUFDmyTbF9QvZL2EFSfCcv+oivyKcDUcbIWAfVfxuFnWferAz7m3KQqDy+dihZfkbsq/8fxRu4OmzuMroZOpmbedLbJqbToGEcx2K9D6H6jyQeLP7dmJ4h1WSTOeLUBIOR+TrHoxQ/VV40/MKx5A0EX1MHY8yja+vH29RgS21vggxV8WFmh6NnmJTfFO/ciCpFxLnmNBCzwompNry1ke0ik8630u3aXgCQIQay+2r4jVm0vkHL3APDXc6dt+YeqgLxJpjO/45d90fwNsrQA2g7jmiDw==";
        let lines = signing_string(&[
            ("date", DATE),
            ("(request-target)", "post /20160918/volumeAttachments"),
            ("host", HOST),
            ("content-length", "316"),
            ("content-type", "application/json"),
            (
                "x-content-sha256",
                "V9Z20UJTvkvpJ50flBzKE32+6m2zJjweHpDMX/U4Uy0=",
            ),
        ]);
        for pem in [
            include_str!("testdata/oci-test-key-pkcs8.pem"),
            include_str!("testdata/oci-test-key-pkcs1.pem"),
        ] {
            let key = SigningKey::from_pem(pem).expect("a key of 2048 bits");
            assert_eq!(key.sign(lines.as_bytes()), OPENSSL);
        }
    }

    #[test]
    fn the_authorization_names_the_key_the_algorithm_and_the_lines() {
        let key =
            SigningKey::from_pem(include_str!("testdata/oci-test-key-pkcs8.pem")).expect("a key");
        let authorization = authorization(
            &key,
            "ocid1.tenancy.oc1..t/ocid1.user.oc1..u/73:61:a2",
            &[
                ("date", DATE),
                ("(request-target)", "post /x"),
                ("host", HOST),
            ],
        );
        let signature = key.sign(
            signing_string(&[
                ("date", DATE),
                ("(request-target)", "post /x"),
                ("host", HOST),
            ])
            .as_bytes(),
        );
        assert_eq!(
            authorization,
            format!(
                "Signature version=\"1\",keyId=\"ocid1.tenancy.oc1..t/ocid1.user.oc1..u/73:61:a2\",\
                 algorithm=\"rsa-sha256\",headers=\"date (request-target) host\",\
                 signature=\"{signature}\""
            )
        );
    }

    #[test]
    fn an_encrypted_or_other_key_is_refused_without_its_contents() {
        for pem in [
            "-----BEGIN ENCRYPTED PRIVATE KEY-----\nMIIC\n-----END ENCRYPTED PRIVATE KEY-----\n",
            "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n\nMIIC\n-----END RSA PRIVATE KEY-----\n",
            "-----BEGIN CERTIFICATE-----\nMIIC\n-----END CERTIFICATE-----\n",
            "not a key",
        ] {
            let refused = SigningKey::from_pem(pem).expect_err("no usable key");
            assert!(!refused.to_string().contains("MIIC"), "{refused}");
        }
    }
}
