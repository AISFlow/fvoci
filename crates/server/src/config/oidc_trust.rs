//! Additional OIDC trust roots, loaded once when the server starts its client.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, bail};
use tokio_rustls::rustls::RootCertStore;
use tokio_rustls::rustls::pki_types::{CertificateDer, pem::PemObject as _};

use super::Configuration;

impl Configuration {
    /// The extra roots supplied by configuration and by an in-process caller.
    ///
    /// # Errors
    ///
    /// Refuses an unreadable file, invalid PEM or DER, or a file without certificates.
    pub(crate) fn oidc_trusted_roots(&self) -> anyhow::Result<Vec<Vec<u8>>> {
        let mut roots = self.oidc_extra_root_certificates.clone();
        if !self.oidc_extra_ca_file.is_empty() {
            roots.extend(read(Path::new(&self.oidc_extra_ca_file))?);
        }
        Ok(roots)
    }
}

fn read(path: &Path) -> anyhow::Result<Vec<Vec<u8>>> {
    let pem =
        fs::read(path).with_context(|| format!("reading oidc_extra_ca_file {}", path.display()))?;
    let mut checked = RootCertStore::empty();
    let mut roots = Vec::new();
    for certificate in CertificateDer::pem_slice_iter(&pem) {
        let certificate = certificate.with_context(|| {
            format!("oidc_extra_ca_file {} contains invalid PEM", path.display())
        })?;
        checked.add(certificate.clone()).with_context(|| {
            format!(
                "oidc_extra_ca_file {} contains an invalid certificate",
                path.display()
            )
        })?;
        roots.push(certificate.as_ref().to_vec());
    }
    if roots.is_empty() {
        bail!(
            "oidc_extra_ca_file {} contains no certificates",
            path.display()
        );
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct PemFile(PathBuf);

    impl PemFile {
        fn new(text: &str) -> Self {
            let directory =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/oidc-ca-tests");
            fs::create_dir_all(&directory).expect("the certificate test directory");
            let path = directory.join(format!("{}.pem", fvoci_model::new_id()));
            fs::write(&path, text).expect("the certificate test file");
            Self(path)
        }

        fn config(&self) -> Configuration {
            Configuration {
                oidc_extra_ca_file: self.0.to_string_lossy().into_owned(),
                ..Configuration::default()
            }
        }
    }

    impl Drop for PemFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn no_file_keeps_the_roots_supplied_in_process() {
        let config = Configuration {
            oidc_extra_root_certificates: vec![vec![1, 2, 3]],
            ..Configuration::default()
        };
        assert_eq!(
            config.oidc_trusted_roots().expect("no file"),
            vec![vec![1, 2, 3]]
        );
        assert_eq!(
            Configuration::default()
                .oidc_trusted_roots()
                .expect("defaults"),
            Vec::<Vec<u8>>::new()
        );
    }

    #[test]
    fn every_certificate_in_the_file_is_added() {
        let first = rcgen::generate_simple_self_signed(vec!["first.test".to_owned()])
            .expect("first certificate")
            .cert;
        let second = rcgen::generate_simple_self_signed(vec!["second.test".to_owned()])
            .expect("second certificate")
            .cert;
        let file = PemFile::new(&format!("{}\n{}", first.pem(), second.pem()));
        let mut config = file.config();
        config.oidc_extra_root_certificates = vec![vec![7]];
        assert_eq!(
            config.oidc_trusted_roots().expect("two roots"),
            vec![
                vec![7],
                first.der().as_ref().to_vec(),
                second.der().as_ref().to_vec()
            ]
        );
    }

    #[test]
    fn the_environment_overrides_the_file_path() {
        let config = Configuration::parse(r#"{"oidc_extra_ca_file":"file.pem"}"#, |name| {
            (name == "FVOCI_OIDC_EXTRA_CA_FILE").then(|| "environment.pem".to_owned())
        })
        .expect("a path setting");
        assert_eq!(config.oidc_extra_ca_file, "environment.pem");
    }

    #[test]
    fn an_unreadable_file_is_refused() {
        let file = PemFile::new("");
        fs::remove_file(&file.0).expect("remove the test file");
        let error = file
            .config()
            .oidc_trusted_roots()
            .expect_err("missing file");
        assert!(
            error.to_string().contains("reading oidc_extra_ca_file"),
            "{error}"
        );
    }

    #[test]
    fn empty_or_invalid_certificate_files_are_refused() {
        for text in [
            "",
            "not a certificate",
            "-----BEGIN CERTIFICATE-----\n!\n-----END CERTIFICATE-----",
            "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----",
        ] {
            let file = PemFile::new(text);
            let error = file
                .config()
                .oidc_trusted_roots()
                .expect_err(&format!("accepted {text:?}"));
            // The message names the file, so a refused start says which one.
            assert!(
                format!("{error:#}").contains(&file.0.display().to_string()),
                "{error:#}"
            );
        }
    }
}
