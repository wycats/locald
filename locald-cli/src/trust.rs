use crate::style;
use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, IsCa, KeyPair, KeyUsagePurpose,
};
use std::path::PathBuf;

pub fn run() -> Result<()> {
    let certs_dir = get_certs_dir()?;
    std::fs::create_dir_all(&certs_dir).context("Failed to create certs directory")?;

    let ca_cert_path = certs_dir.join("rootCA.pem");
    let ca_key_path = certs_dir.join("rootCA-key.pem");

    if !ca_cert_path.exists() || !ca_key_path.exists() {
        println!("Generating new Root CA...");
        generate_ca(&ca_cert_path, &ca_key_path)?;
        println!(
            "{} Root CA generated at {}",
            style::CHECK,
            ca_cert_path.display()
        );
    } else {
        println!("Root CA already exists at {}", ca_cert_path.display());
    }

    println!("Installing Root CA to system trust store...");
    install_ca(&ca_cert_path)?;
    println!("{} Root CA installed successfully.", style::CHECK);

    Ok(())
}

fn get_certs_dir() -> Result<PathBuf> {
    let home = home::home_dir().context("Could not find home directory")?;
    Ok(home.join(".locald").join("certs"))
}

fn generate_ca(cert_path: &std::path::Path, key_path: &std::path::Path) -> Result<()> {
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, "locald Development CA");
    dn.push(rcgen::DnType::OrganizationName, "locald");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;

    std::fs::write(cert_path, cert.pem())?;
    std::fs::write(key_path, key_pair.serialize_pem())?;

    Ok(())
}

fn install_ca(cert_path: &std::path::Path) -> Result<()> {
    let path_str = cert_path.to_str().context("Invalid path string")?;
    if let Err(e) = ca_injector::install_ca(path_str) {
        if e.to_string().contains("Permission denied") || e.to_string().contains("os error 13") {
            anyhow::bail!(
                "Permission denied. Please run `sudo locald trust` to install the certificate."
            );
        }
        return Err(e).context("Failed to install CA certificate");
    }
    Ok(())
}
