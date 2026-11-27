//! AWS Signature Version 4, as Amazon SES's API v2 reads it: HMAC-SHA256 over the
//! canonical request, with a key derived from the secret, the day, the region and the
//! service.
//! <https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv-create-signed-request.html>

use std::fmt::Write as _;

use aws_lc_rs::hmac;
use sha2::{Digest as _, Sha256};

use super::clock::Moment;
use crate::auth::secret::token_digest;

/// The algorithm a `SigV4` signature names.
const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// Who signs, and for what.
pub(super) struct Signer<'a> {
    pub(super) access_key_id: &'a str,
    pub(super) secret_access_key: &'a str,
    pub(super) region: &'a str,
    pub(super) service: &'a str,
}

/// What of a request is signed. `path` is written as it goes out (already encoded), and
/// the query is empty: neither of SES's requests has one.
pub(super) struct Request<'a> {
    pub(super) method: &'a str,
    pub(super) path: &'a str,
    /// Every header that is signed, `host` and `x-amz-date` among them, with names in
    /// any case.
    pub(super) headers: &'a [(&'a str, &'a str)],
    pub(super) body: &'a [u8],
}

/// The lower-case hexadecimal of `bytes`.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

fn sign(key: &[u8], data: &str) -> hmac::Tag {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data.as_bytes())
}

impl Request<'_> {
    /// The signed headers' names, lower case, sorted, and the canonical request.
    fn canonical(&self) -> (String, String) {
        let mut headers: Vec<(String, String)> = self
            .headers
            .iter()
            .map(|(name, value)| {
                // Values are trimmed, and runs of spaces inside them are one space.
                let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
                (name.to_ascii_lowercase(), value)
            })
            .collect();
        headers.sort();
        let signed = headers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let canonical_headers = headers
            .iter()
            .fold(String::new(), |mut text, (name, value)| {
                let _ = writeln!(text, "{name}:{value}");
                text
            });
        let canonical = format!(
            "{}\n{}\n\n{canonical_headers}\n{signed}\n{}",
            self.method,
            self.path,
            hex(&Sha256::digest(self.body))
        );
        (signed, canonical)
    }
}

impl Signer<'_> {
    /// The scope of a signature made on `day` (`20150830`).
    fn scope(&self, day: &str) -> String {
        format!("{day}/{}/{}/aws4_request", self.region, self.service)
    }

    /// The string to sign of `canonical`, for a request made `at`.
    fn string_to_sign(&self, at: Moment, canonical: &str) -> String {
        format!(
            "{ALGORITHM}\n{}\n{}\n{}",
            at.amz_date(),
            self.scope(&at.amz_day()),
            token_digest(canonical)
        )
    }

    /// The signature of `string_to_sign` on `day`.
    fn signature(&self, day: &str, string_to_sign: &str) -> String {
        let secret = format!("AWS4{}", self.secret_access_key);
        let date = sign(secret.as_bytes(), day);
        let region = sign(date.as_ref(), self.region);
        let service = sign(region.as_ref(), self.service);
        let signing = sign(service.as_ref(), "aws4_request");
        hex(sign(signing.as_ref(), string_to_sign).as_ref())
    }

    /// The `Authorization` header of `request`, made `at`: the request's `X-Amz-Date`
    /// is `at.amz_date()`.
    pub(super) fn authorization(&self, request: &Request<'_>, at: Moment) -> String {
        let (signed, canonical) = request.canonical();
        let string_to_sign = self.string_to_sign(at, &canonical);
        let day = at.amz_day();
        format!(
            "{ALGORITHM} Credential={}/{}, SignedHeaders={signed}, Signature={}",
            self.access_key_id,
            self.scope(&day),
            self.signature(&day, &string_to_sign)
        )
    }
}

/// The cases of AWS's signature test suite that sign a POST to `/` without a query,
/// as SES's requests are: their inputs, and the canonical request, the string to sign
/// and the `Authorization` header the suite gives for them.
/// <https://github.com/awslabs/aws-c-auth/tree/main/tests/aws-signing-test-suite/v4>
/// (`post-vanilla`, `post-x-www-form-urlencoded`; read 2026-11-14).
#[cfg(test)]
mod tests {
    use super::*;

    const SIGNER: Signer<'static> = Signer {
        access_key_id: "AKIDEXAMPLE",
        secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        region: "us-east-1",
        service: "service",
    };
    const AMZ_DATE: &str = "20150830T123600Z";
    /// The suite's time, 2015-08-30T12:36:00Z.
    const AT: u64 = 1_440_938_160;

    fn check(request: &Request<'_>, canonical: &str, string_to_sign: &str, authorization: &str) {
        let (_, made) = request.canonical();
        assert_eq!(made, canonical, "the canonical request");
        assert_eq!(Moment::at(AT).amz_date(), AMZ_DATE);
        assert_eq!(
            SIGNER.string_to_sign(Moment::at(AT), &made),
            string_to_sign,
            "the string to sign"
        );
        assert_eq!(SIGNER.authorization(request, Moment::at(AT)), authorization);
    }

    #[test]
    fn post_vanilla_signs_as_the_suite_says() {
        let request = Request {
            method: "POST",
            path: "/",
            headers: &[("Host", "example.amazonaws.com"), ("X-Amz-Date", AMZ_DATE)],
            body: b"",
        };
        check(
            &request,
            "POST\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\n\
             host;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n\
             553f88c9e4d10fc9e109e2aeb65f030801b70c2f6468faca261d401ae622fc87",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b",
        );
    }

    #[test]
    fn post_x_www_form_urlencoded_signs_its_body_as_the_suite_says() {
        let request = Request {
            method: "POST",
            path: "/",
            headers: &[
                ("Content-Type", "application/x-www-form-urlencoded"),
                ("Host", "example.amazonaws.com"),
                ("Content-Length", "13"),
                ("X-Amz-Date", AMZ_DATE),
                (
                    "x-amz-content-sha256",
                    "9095672bbd1f56dfc5b65f3e153adc8731a4a654192329106275f4c7b24d0b6e",
                ),
            ],
            body: b"Param1=value1",
        };
        check(
            &request,
            "POST\n/\n\ncontent-length:13\ncontent-type:application/x-www-form-urlencoded\n\
             host:example.amazonaws.com\n\
             x-amz-content-sha256:9095672bbd1f56dfc5b65f3e153adc8731a4a654192329106275f4c7b24d0b6e\n\
             x-amz-date:20150830T123600Z\n\n\
             content-length;content-type;host;x-amz-content-sha256;x-amz-date\n\
             9095672bbd1f56dfc5b65f3e153adc8731a4a654192329106275f4c7b24d0b6e",
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n\
             b1edd1d03544c25390e32085d55b57acc9a3961bb59415ff86c45c3d89d16cfb",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=content-length;content-type;host;x-amz-content-sha256;x-amz-date, \
             Signature=d3875051da38690788ef43de4db0d8f280229d82040bfac253562e56c3f20e0b",
        );
    }
}
