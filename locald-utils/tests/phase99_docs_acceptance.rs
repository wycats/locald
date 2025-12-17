#![allow(missing_docs)]

#[test]
fn phase99_acceptance_mentions_name_sanitization() {
    // This test is intentionally opinionated: it prevents doc drift between the
    // RFC/manual acceptance criteria and the implemented cgroup path generator,
    // which sanitizes components to avoid ':' / '..' / empty path segments.
    let rfc = include_str!("../../docs/rfcs/0099-cgroup-hierarchy.md");
    let manual = include_str!("../../docs/manual/architecture/resource-management.md");

    assert!(
        rfc.contains("sanitiz") || rfc.contains("sanitize"),
        "RFC 0099 should explicitly mention name sanitization for cgroup paths"
    );
    assert!(
        manual.contains("sanitiz") || manual.contains("sanitize"),
        "Resource management manual should mention name sanitization for cgroup paths"
    );
}
