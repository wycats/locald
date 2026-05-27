mod backend;
mod status;

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    backend::macos::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("locald-agent is macOS-only");
}
