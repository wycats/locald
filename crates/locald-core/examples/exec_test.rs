//! Minimal example for parsing an `exec` service config.

use locald_core::config::{LocaldConfig, ServiceConfig, TypedServiceConfig};

fn main() {
    let toml_str = r#"
[project]
name = "color-system"

[services.web]
type = "exec"
command = "pnpm docs:dev"
"#;

    let config: LocaldConfig = toml::from_str(toml_str).unwrap();
    println!("{:#?}", config);

    let service = &config.services["web"];
    match service {
        ServiceConfig::Typed(TypedServiceConfig::Exec(c)) => {
            println!("Exec service. Build config: {:?}", c.build);
            if c.build.is_some() {
                println!("FAIL: Build config should be None");
            } else {
                println!("PASS: Build config is None");
            }
        }
        _ => println!("Unexpected service type"),
    }
}
