//! The run's CA, the certificates it signs for bound hosts, the trust bundle
//! the command is pointed at, and how porta verifies the real server.
//!
//! Everything runs on `ring`, the provider reqwest's rustls already builds:
//! no second crypto library, and no OpenSSL.

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A CA made for one run. It lives in memory; its certificate alone is
/// written out, for the command to trust.
pub(super) struct Authority {
    cert: rcgen::Certificate,
    key: rcgen::KeyPair,
}

impl Authority {
    pub(super) fn new() -> Result<Authority, String> {
        let mut params = rcgen::CertificateParams::default();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.distinguished_name.push(rcgen::DnType::CommonName, "porta run CA (this run only)");
        params.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign, rcgen::KeyUsagePurpose::DigitalSignature];
        set_validity(&mut params);
        let key = rcgen::KeyPair::generate().map_err(|e| format!("CA key: {e}"))?;
        let cert = params.self_signed(&key).map_err(|e| format!("CA: {e}"))?;
        Ok(Authority { cert, key })
    }

    /// Writes the CA certificate, and a bundle of the system's roots plus the
    /// CA, into `dir`. SSL_CERT_FILE and the variables like it replace the
    /// trust store rather than add to it, so what they point at has to hold the
    /// system's roots as well, or every other host would stop verifying. No
    /// bundle when the system's roots cannot be found.
    pub(super) fn write_trust(&self, dir: &str) -> Result<(String, Option<String>), String> {
        let ca_path = format!("{dir}/ca.pem");
        std::fs::write(&ca_path, self.cert.pem()).map_err(|e| format!("cannot write {ca_path}: {e}"))?;
        let Some(mut bundle) = system_roots() else { return Ok((ca_path, None)) };
        if !bundle.ends_with('\n') {
            bundle.push('\n');
        }
        bundle.push_str(&self.cert.pem());
        let bundle_path = format!("{dir}/bundle.pem");
        std::fs::write(&bundle_path, bundle).map_err(|e| format!("cannot write {bundle_path}: {e}"))?;
        Ok((ca_path, Some(bundle_path)))
    }

    /// A TLS server configuration presenting a certificate for `host`, signed
    /// by this CA.
    pub(super) fn server_config_for(&self, host: &str) -> Result<rustls::ServerConfig, String> {
        let mut params = rcgen::CertificateParams::new(vec![host.to_string()]).map_err(|e| format!("leaf for {host}: {e}"))?;
        params.distinguished_name.push(rcgen::DnType::CommonName, host);
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        // Strict verifiers (Python 3.13 and later, by default) refuse a leaf
        // that does not say which key signed it.
        params.use_authority_key_identifier_extension = true;
        set_validity(&mut params);
        let key = rcgen::KeyPair::generate().map_err(|e| format!("leaf key: {e}"))?;
        let leaf = params.signed_by(&key, &self.cert, &self.key).map_err(|e| format!("leaf for {host}: {e}"))?;
        let chain = vec![leaf.der().clone(), self.cert.der().clone()];
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let mut config = rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| format!("server config: {e}"))?
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .map_err(|e| format!("server config: {e}"))?;
        // HTTP/1.1 alone: the proxy reads one request head and one body, and
        // a client that would speak HTTP/2 falls back when it is not offered.
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(config)
    }
}

/// From yesterday to a week from now: long enough for any run, short enough
/// that a leaked certificate is soon worthless, and within the lifetimes
/// clients accept for a server certificate.
fn set_validity(params: &mut rcgen::CertificateParams) {
    let days = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() / 86400).unwrap_or(0) as i64;
    let (y1, m1, d1) = civil_from_days(days - 1);
    let (y2, m2, d2) = civil_from_days(days + 7);
    params.not_before = rcgen::date_time_ymd(y1, m1, d1);
    params.not_after = rcgen::date_time_ymd(y2, m2, d2);
}

/// Days since 1970-01-01 as a calendar date (Howard Hinnant's algorithm).
pub(super) fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u8, day as u8)
}

/// The system's trust store as PEM: the host's own SSL_CERT_FILE if it has
/// one, else the first bundle a distribution keeps in its usual place.
fn system_roots() -> Option<String> {
    let mut candidates: Vec<String> = std::env::var("SSL_CERT_FILE").into_iter().collect();
    candidates.extend(
        ["/etc/ssl/cert.pem", "/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/ca-bundle.pem"]
            .iter()
            .map(|s| s.to_string()),
    );
    candidates.iter().find_map(|path| std::fs::read_to_string(path).ok().filter(|text| text.contains("BEGIN CERTIFICATE")))
}

/// How porta verifies the real server: the webpki roots, plus, for the
/// integration suite alone, the CA named by PORTA_TEST_UPSTREAM_CA in porta's
/// own environment.
pub(super) fn upstream_config() -> Result<Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Ok(path) = std::env::var("PORTA_TEST_UPSTREAM_CA") {
        add_test_roots(&mut roots, &path)?;
    }
    let mut config = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("client config: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

fn add_test_roots(roots: &mut rustls::RootCertStore, path: &str) -> Result<(), String> {
    let certs = CertificateDer::pem_file_iter(path).map_err(|e| format!("PORTA_TEST_UPSTREAM_CA {path}: {e}"))?;
    for cert in certs {
        let cert = cert.map_err(|e| format!("PORTA_TEST_UPSTREAM_CA {path}: {e}"))?;
        roots.add(cert).map_err(|e| format!("PORTA_TEST_UPSTREAM_CA {path}: {e}"))?;
    }
    Ok(())
}
