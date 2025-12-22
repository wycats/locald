
#[cfg(test)]
mod tests {
    use portable_pty::{NativePtySystem, PtySize, PtySystem};
    use std::io::Write;

    #[test]
    fn test_pty_write() {
        let system = NativePtySystem::default();
        let pair = system.openpty(PtySize::default()).unwrap();
        let mut master = pair.master;
        
        // Try to write
        master.write_all(b"hello").unwrap();
    }
}
