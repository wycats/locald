use locald_core::{DomainName, SharedDomainIndex};

/// Return the canonical concrete hostname when the live domain index owns it.
///
/// The snapshot is loaded for every TLS handshake so new claims become
/// available immediately and removed claims cannot reuse cached certificates.
// The private module keeps this internal, while parent and sibling modules
// need crate visibility; `pub` would violate the crate's `unreachable_pub` lint.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn owned_server_name(
    domain_index: &SharedDomainIndex,
    requested_server_name: &str,
) -> Option<String> {
    let domain = requested_server_name.parse::<DomainName>().ok()?;
    let snapshot = domain_index.snapshot();
    snapshot.resolve(domain.as_str())?;
    Some(domain.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use locald_core::{DomainClaim, DomainIndex, DomainPattern, ProjectInstanceId};
    use uuid::Uuid;

    fn instance(seed: u128) -> ProjectInstanceId {
        Uuid::from_u128(seed)
            .to_string()
            .parse()
            .expect("valid project instance UUID")
    }

    #[test]
    fn only_concrete_owned_names_are_canonicalized_and_authorized() {
        let domains = SharedDomainIndex::default();

        assert_eq!(
            owned_server_name(&domains, "DOCS.LOCALHOST."),
            Some("docs.localhost".to_owned())
        );
        assert!(owned_server_name(&domains, "telemetry.vercel.com").is_none());
        assert!(owned_server_name(&domains, "*.localhost").is_none());
        assert!(owned_server_name(&domains, "127.0.0.1").is_none());
        assert!(owned_server_name(&domains, "münchen.localhost").is_none());
        assert!(owned_server_name(&domains, "").is_none());
    }

    #[test]
    fn authorization_reads_each_complete_shared_snapshot() {
        let instance_id = instance(1);
        let first = DomainIndex::default()
            .replacing_instance(
                instance_id,
                [DomainClaim::service(
                    "first.localhost".parse().expect("valid domain"),
                    instance_id,
                    "app:web".to_owned(),
                )],
            )
            .expect("install first claim");
        let domains = SharedDomainIndex::new(first);

        assert_eq!(
            owned_server_name(&domains, "first.localhost"),
            Some("first.localhost".to_owned())
        );
        assert!(owned_server_name(&domains, "second.localhost").is_none());

        let second = domains
            .snapshot()
            .replacing_instance(
                instance_id,
                [DomainClaim::service(
                    "second.localhost".parse().expect("valid domain"),
                    instance_id,
                    "app:web".to_owned(),
                )],
            )
            .expect("replace claim");
        domains.store(second);

        assert!(owned_server_name(&domains, "first.localhost").is_none());
        assert_eq!(
            owned_server_name(&domains, "SECOND.LOCALHOST."),
            Some("second.localhost".to_owned())
        );
    }

    #[test]
    fn concrete_one_label_wildcard_names_are_authorized() {
        let instance_id = instance(2);
        let index = DomainIndex::default()
            .replacing_instance(
                instance_id,
                [
                    DomainClaim::service(
                        "frame.app.localhost".parse().expect("valid frame origin"),
                        instance_id,
                        "app:frame".to_owned(),
                    ),
                    DomainClaim::service_pattern(
                        DomainPattern::wildcard(
                            "frame.app.localhost"
                                .parse()
                                .expect("valid wildcard suffix"),
                        ),
                        instance_id,
                        "app:frame".to_owned(),
                        false,
                    ),
                ],
            )
            .expect("install wildcard claim");
        let domains = SharedDomainIndex::new(index);

        assert_eq!(
            owned_server_name(&domains, "PREVIEW.frame.app.localhost."),
            Some("preview.frame.app.localhost".to_owned())
        );
        assert_eq!(
            owned_server_name(&domains, "frame.app.localhost"),
            Some("frame.app.localhost".to_owned())
        );
        assert!(owned_server_name(&domains, "deep.preview.frame.app.localhost").is_none());
        assert!(owned_server_name(&domains, "unowned.app.localhost").is_none());
    }
}
