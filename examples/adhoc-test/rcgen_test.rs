use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, KeyUsagePurpose,
};

fn main() {
    let mut params = CertificateParams::default();
    let key_pair = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key_pair).unwrap();

    let mut child_params = CertificateParams::default();
    let child_key = KeyPair::generate().unwrap();

    // Try to sign
    // let child_cert = child_params.signed_by(&child_key, &cert, &key_pair).unwrap(); // Old API?
    // let child_cert = child_params.signed_by(&child_key, &cert).unwrap(); // New API?
}
