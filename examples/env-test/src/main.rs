fn main() {
    for (key, value) in std::env::vars() {
        if key.starts_with("LD_") {
            println!("{}={}", key, value);
        }
    }
}
