use rcgen::{CertificateParams, CertifiedIssuer, KeyPair};

fn main() {
    let params = CertificateParams::default();
    let key_pair = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key_pair).unwrap();

    let child_params = CertificateParams::default();
    let child_key = KeyPair::generate().unwrap();

    // Try to sign
    // let _ = child_params.signed_by(&child_key, &cert, &key_pair);
    let issuer = CertifiedIssuer::self_signed(params, key_pair).unwrap();
    let _ = child_params.signed_by(&child_key, &issuer);
}
