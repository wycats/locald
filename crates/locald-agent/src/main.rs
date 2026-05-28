mod backend;
mod status;

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    backend::macos::run()
}

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    backend::linux::run()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    println!("locald-agent is unsupported on this platform");
}
