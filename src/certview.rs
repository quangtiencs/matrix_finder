//! Certificate previews: what a PEM or DER file holds. Private keys are only named,
//! never shown, as the preview is often on screen while other people can see it.
use crate::i18n::t;
use crate::util::ext_of;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use x509_parser::prelude::*;
use x509_parser::public_key::PublicKey;

pub fn is_cert(name: &str) -> bool {
    matches!(ext_of(name).as_str(), "pem" | "crt" | "cer" | "der")
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Head(String),
    Field(String, String),
    Good(String, String),
    Bad(String, String),
    Note(String),
}

/// What a certificate file contains, as rows to show. `Err` if it holds nothing recognisable.
pub fn load(bytes: &[u8]) -> Result<(String, Vec<Row>), String> {
    let now = chrono::Utc::now().timestamp();
    load_at(bytes, now)
}

fn load_at(bytes: &[u8], now: i64) -> Result<(String, Vec<Row>), String> {
    let mut rows = vec![];
    let mut certs = 0;
    let mut others = 0;
    let mut is_pem = false;
    for pem in Pem::iter_from_buffer(bytes) {
        let Ok(pem) = pem else { break };
        is_pem = true;
        if pem.label == "CERTIFICATE" || pem.label == "TRUSTED CERTIFICATE" || pem.label == "X509 CERTIFICATE" {
            certs += 1;
            rows.push(Row::Head(format!("{} {certs}", t("Certificate"))));
            match X509Certificate::from_der(&pem.contents) {
                Ok((_, c)) => cert_rows(&c, &pem.contents, now, &mut rows),
                Err(e) => rows.push(Row::Bad(t("Error"), e.to_string())),
            }
        } else {
            others += 1;
            rows.push(Row::Head(block_name(&pem.label)));
            if pem.label.contains("PRIVATE KEY") {
                rows.push(Row::Note(t("Contains a private key. Its contents are not shown; keep this file secret.")));
            }
        }
    }
    if !is_pem {
        let (_, c) = X509Certificate::from_der(bytes).map_err(|_| t("Not a certificate file"))?;
        rows.push(Row::Head(t("Certificate")));
        cert_rows(&c, bytes, now, &mut rows);
        return Ok(("X.509 · DER".to_string(), rows));
    }
    if certs + others == 0 {
        return Err(t("Not a certificate file"));
    }
    let format = if certs > 0 { format!("X.509 · PEM · {certs} {}", t("certificate(s)")) } else { "PEM".to_string() };
    Ok((format, rows))
}

fn block_name(label: &str) -> String {
    match label {
        "PRIVATE KEY" | "RSA PRIVATE KEY" | "EC PRIVATE KEY" | "DSA PRIVATE KEY" | "OPENSSH PRIVATE KEY" | "ENCRYPTED PRIVATE KEY" => t("Private key"),
        "PUBLIC KEY" | "RSA PUBLIC KEY" => t("Public key"),
        "CERTIFICATE REQUEST" | "NEW CERTIFICATE REQUEST" => t("Certificate request"),
        "X509 CRL" => t("Certificate revocation list"),
        other => other.to_string(),
    }
}

fn fingerprint(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

fn date(seconds: i64) -> String {
    chrono::DateTime::from_timestamp(seconds, 0).map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default()
}

fn cert_rows(c: &X509Certificate, der: &[u8], now: i64, rows: &mut Vec<Row>) {
    rows.push(Row::Field(t("Subject"), c.subject().to_string()));
    rows.push(Row::Field(t("Issuer"), if c.subject() == c.issuer() { format!("{} ({})", c.issuer(), t("self-signed")) } else { c.issuer().to_string() }));
    let (from, to) = (c.validity().not_before.timestamp(), c.validity().not_after.timestamp());
    let days = (to - now) / 86400;
    rows.push(if now < from {
        Row::Bad(t("Status"), format!("{} {}", t("Not valid until"), date(from)))
    } else if now > to {
        Row::Bad(t("Status"), format!("{} {} {}", t("Expired"), -days, t("days ago")))
    } else {
        Row::Good(t("Status"), format!("{} {days} {}", t("Valid for"), t("more days")))
    });
    rows.push(Row::Field(t("Valid from"), date(from)));
    rows.push(Row::Field(t("Valid until"), date(to)));
    let mut names = vec![];
    if let Ok(Some(san)) = c.subject_alternative_name() {
        for n in &san.value.general_names {
            match n {
                GeneralName::DNSName(d) => names.push((*d).to_string()),
                GeneralName::RFC822Name(m) => names.push((*m).to_string()),
                GeneralName::URI(u) => names.push((*u).to_string()),
                GeneralName::IPAddress(ip) => names.push(format_ip(ip)),
                _ => {}
            }
        }
    }
    if !names.is_empty() {
        rows.push(Row::Field(t("Other names"), names.join(", ")));
    }
    rows.push(Row::Field(t("Public key"), public_key_name(c)));
    rows.push(Row::Field(t("Signature"), signature_name(c)));
    if let Ok(Some(b)) = c.basic_constraints() {
        rows.push(Row::Field(t("Certificate authority"), if b.value.ca { t("Yes") } else { t("No") }));
    }
    if let Ok(Some(k)) = c.key_usage() {
        rows.push(Row::Field(t("Key usage"), k.value.to_string()));
    }
    if let Ok(Some(k)) = c.extended_key_usage() {
        let e = k.value;
        let uses = [(e.server_auth, "server"), (e.client_auth, "client"), (e.code_signing, "code signing"), (e.email_protection, "email"), (e.time_stamping, "time stamping"), (e.ocsp_signing, "OCSP")];
        let list: Vec<&str> = uses.iter().filter(|(on, _)| *on).map(|(_, n)| *n).collect();
        if !list.is_empty() {
            rows.push(Row::Field(t("Used for"), list.join(", ")));
        }
    }
    rows.push(Row::Field(t("Serial number"), c.raw_serial_as_string().to_uppercase()));
    rows.push(Row::Field("SHA-256".to_string(), fingerprint(&Sha256::digest(der))));
    rows.push(Row::Field("SHA-1".to_string(), fingerprint(&Sha1::digest(der))));
}

fn format_ip(ip: &[u8]) -> String {
    match ip.len() {
        4 => ip.iter().map(u8::to_string).collect::<Vec<_>>().join("."),
        16 => {
            let groups: [u8; 16] = ip.try_into().unwrap_or_default();
            std::net::Ipv6Addr::from(groups).to_string()
        }
        _ => fingerprint(ip),
    }
}

fn public_key_name(c: &X509Certificate) -> String {
    match c.public_key().parsed() {
        Ok(PublicKey::RSA(k)) => format!("RSA {} bit", k.key_size()),
        Ok(PublicKey::EC(k)) => format!("EC {} bit", k.key_size()),
        Ok(PublicKey::DSA(_)) => "DSA".to_string(),
        Ok(PublicKey::GostR3410(_) | PublicKey::GostR3410_2012(_)) => "GOST".to_string(),
        _ => oid_name(&c.public_key().algorithm.algorithm.to_id_string()),
    }
}

fn signature_name(c: &X509Certificate) -> String {
    oid_name(&c.signature_algorithm.algorithm.to_id_string())
}

fn oid_name(oid: &str) -> String {
    match oid {
        "1.2.840.113549.1.1.5" => "RSA with SHA-1",
        "1.2.840.113549.1.1.11" => "RSA with SHA-256",
        "1.2.840.113549.1.1.12" => "RSA with SHA-384",
        "1.2.840.113549.1.1.13" => "RSA with SHA-512",
        "1.2.840.113549.1.1.10" => "RSA-PSS",
        "1.2.840.113549.1.1.1" => "RSA",
        "1.2.840.10045.4.3.2" => "ECDSA with SHA-256",
        "1.2.840.10045.4.3.3" => "ECDSA with SHA-384",
        "1.2.840.10045.4.3.4" => "ECDSA with SHA-512",
        "1.3.101.112" => "Ed25519",
        "1.3.101.113" => "Ed448",
        other => other,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -days 3650 -subj /CN=test.example -addext subjectAltName=DNS:test.example,IP:127.0.0.1`
    const PEM: &str = include_str!("../assets/testdata/test-cert.pem");

    fn value<'a>(rows: &'a [Row], label: &str) -> Option<&'a str> {
        rows.iter().find_map(|r| match r {
            Row::Field(l, v) | Row::Good(l, v) | Row::Bad(l, v) if l == label => Some(v.as_str()),
            _ => None,
        })
    }

    fn der_of_pem() -> Vec<u8> {
        Pem::iter_from_buffer(PEM.as_bytes()).next().unwrap().unwrap().contents
    }

    #[test]
    fn certificate_facts() {
        let (format, rows) = load_at(PEM.as_bytes(), 1_800_000_000).unwrap();
        assert!(format.contains("PEM"), "{format}");
        assert_eq!(value(&rows, "Subject"), Some("CN=test.example"));
        assert!(value(&rows, "Issuer").unwrap().contains("self-signed"));
        assert_eq!(value(&rows, "Other names"), Some("test.example, 127.0.0.1"));
        assert_eq!(value(&rows, "Public key"), Some("EC 256 bit"));
        assert_eq!(value(&rows, "Signature"), Some("ECDSA with SHA-256"));
        assert_eq!(value(&rows, "SHA-256").unwrap().len(), 32 * 3 - 1);
        assert!(rows.iter().any(|r| matches!(r, Row::Good(..))), "valid at that time");
    }

    #[test]
    fn expiry_is_called_out() {
        let (_, rows) = load_at(PEM.as_bytes(), 4_000_000_000).unwrap();
        assert!(rows.iter().any(|r| matches!(r, Row::Bad(l, v) if l == "Status" && v.contains("Expired"))));
        let (_, rows) = load_at(PEM.as_bytes(), 1_000_000_000).unwrap();
        assert!(rows.iter().any(|r| matches!(r, Row::Bad(l, v) if l == "Status" && v.contains("Not valid until"))));
    }

    #[test]
    fn der_files() {
        let (format, rows) = load_at(&der_of_pem(), 1_800_000_000).unwrap();
        assert!(format.contains("DER"));
        assert_eq!(value(&rows, "Subject"), Some("CN=test.example"));
    }

    #[test]
    fn private_keys_are_named_but_never_shown() {
        let secret = "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgabcdefghijklmnop";
        let pem = format!("-----BEGIN PRIVATE KEY-----\n{secret}\n-----END PRIVATE KEY-----\n");
        let (format, rows) = load_at(pem.as_bytes(), 0).unwrap();
        assert_eq!(format, "PEM");
        assert_eq!(rows[0], Row::Head("Private key".into()));
        assert!(!format!("{rows:?}").contains(&secret[..20]));
    }

    #[test]
    fn other_files_are_refused() {
        assert!(load_at(b"just some text", 0).is_err());
        assert!(load_at(&[0x30, 0x82, 0xFF], 0).is_err());
        assert!(load_at(b"", 0).is_err());
    }
}
