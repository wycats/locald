pub trait BuildProgress: Send + Sync {
    fn phase_started(&self, phase: &str);
    fn phase_output(&self, phase: &str, output: &str);
    fn phase_completed(&self, phase: &str);
    fn phase_failed(&self, phase: &str, error: &str);
}

// A no-op implementation for testing or when no UI is attached
#[derive(Debug, Copy, Clone)]
pub struct NoOpProgress;
impl BuildProgress for NoOpProgress {
    fn phase_started(&self, _phase: &str) {}
    fn phase_output(&self, _phase: &str, _output: &str) {}
    fn phase_completed(&self, _phase: &str) {}
    fn phase_failed(&self, _phase: &str, _error: &str) {}
}
