#[cfg(test)]
mod tests {
    use crate::config::{LocaldConfig, ServiceConfig, TypedServiceConfig};
    use toml;

    #[test]
    fn test_worker_config_deserialization() {
        let toml = r#"
[project]
name = "worker-test"

[services.worker]
type = "worker"
command = "echo working"
"#;
        let config: LocaldConfig = toml::from_str(toml).unwrap();
        let service = config.services.get("worker").unwrap();

        match service {
            ServiceConfig::Typed(TypedServiceConfig::Worker(_)) => {
                println!("Successfully parsed as Worker");
            }
            ServiceConfig::Typed(TypedServiceConfig::Exec(_)) => {
                panic!("Parsed as Exec (Typed)");
            }
            ServiceConfig::Legacy(_) => {
                panic!("Parsed as Legacy (Exec)");
            }
            _ => panic!("Parsed as something else"),
        }
    }

    #[test]
    fn test_container_config_deserialization() {
        let toml = r#"
[project]
name = "container-test"

[services.redis]
type = "container"
image = "redis:7"
container_port = 6379
"#;
        let config: LocaldConfig = toml::from_str(toml).unwrap();
        let service = config.services.get("redis").unwrap();

        match service {
            ServiceConfig::Typed(TypedServiceConfig::Container(c)) => {
                assert_eq!(c.image, "redis:7");
                assert_eq!(c.container_port, Some(6379));
            }
            _ => panic!("Expected Container config"),
        }
    }
}
