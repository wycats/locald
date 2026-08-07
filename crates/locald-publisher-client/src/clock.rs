use std::fmt;
use std::time::Duration;

use thiserror::Error;

/// Suspend-inclusive monotonic client instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SuspendInstant(Duration);

impl SuspendInstant {
    /// Construct a test or platform instant from elapsed duration.
    #[must_use]
    pub const fn from_duration(duration: Duration) -> Self {
        Self(duration)
    }

    /// Return the raw daemon-lifetime elapsed duration.
    #[must_use]
    pub const fn as_duration(self) -> Duration {
        self.0
    }

    fn checked_add(self, duration: Duration) -> Result<Self, ClockError> {
        self.0
            .checked_add(duration)
            .map(Self)
            .ok_or(ClockError::Overflow)
    }
}

/// Failure to establish the suspend-inclusive scheduling contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ClockError {
    /// The platform cannot provide the required clock.
    #[error("the suspend-inclusive publisher clock is unavailable")]
    Unavailable,
    /// A server duration could not be represented safely.
    #[error("publisher renewal schedule overflowed")]
    Overflow,
    /// The server returned an internally inconsistent schedule.
    #[error("renew_after_ms must not exceed expires_in_ms")]
    InvalidSchedule,
    /// The observed suspend-inclusive clock moved backward.
    #[error("the suspend-inclusive publisher clock regressed")]
    Regressed,
}

/// Cloneable source of suspend-inclusive elapsed time.
pub trait SuspendAwareClock: Send + Sync + fmt::Debug {
    /// Observe the current suspend-inclusive instant.
    ///
    /// # Errors
    ///
    /// Returns [`ClockError`] when the platform cannot provide a trustworthy
    /// suspend-inclusive observation.
    fn now(&self) -> Result<SuspendInstant, ClockError>;
}

/// Production suspend-inclusive clock for Linux and macOS.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSuspendAwareClock;

#[cfg(target_os = "linux")]
impl SuspendAwareClock for SystemSuspendAwareClock {
    #[allow(unsafe_code)]
    fn now(&self) -> Result<SuspendInstant, ClockError> {
        let mut value = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `value` is writable and CLOCK_BOOTTIME has no caller-owned
        // lifetime or pointer preconditions.
        if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &raw mut value) } != 0 {
            return Err(ClockError::Unavailable);
        }
        let seconds = u64::try_from(value.tv_sec).map_err(|_| ClockError::Unavailable)?;
        let nanos = u32::try_from(value.tv_nsec).map_err(|_| ClockError::Unavailable)?;
        Ok(SuspendInstant(Duration::new(seconds, nanos)))
    }
}

#[cfg(target_os = "macos")]
impl SuspendAwareClock for SystemSuspendAwareClock {
    #[allow(unsafe_code)]
    fn now(&self) -> Result<SuspendInstant, ClockError> {
        #[repr(C)]
        struct MachTimebaseInfo {
            numer: u32,
            denom: u32,
        }

        unsafe extern "C" {
            fn mach_continuous_time() -> u64;
            fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
        }

        let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: `info` is a valid writable C representation.
        if unsafe { mach_timebase_info(&raw mut info) } != 0 || info.denom == 0 {
            return Err(ClockError::Unavailable);
        }
        // SAFETY: `mach_continuous_time` has no preconditions.
        let ticks = unsafe { mach_continuous_time() };
        let nanos = u128::from(ticks)
            .checked_mul(u128::from(info.numer))
            .and_then(|value| value.checked_div(u128::from(info.denom)))
            .ok_or(ClockError::Unavailable)?;
        let seconds = u64::try_from(nanos / 1_000_000_000).map_err(|_| ClockError::Unavailable)?;
        let subsecond =
            u32::try_from(nanos % 1_000_000_000).map_err(|_| ClockError::Unavailable)?;
        Ok(SuspendInstant(Duration::new(seconds, subsecond)))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl SuspendAwareClock for SystemSuspendAwareClock {
    fn now(&self) -> Result<SuspendInstant, ClockError> {
        Err(ClockError::Unavailable)
    }
}

/// Conservative client schedule anchored to request start, not response receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenewalSchedule {
    renew_at: SuspendInstant,
    expires_at: SuspendInstant,
}

impl RenewalSchedule {
    /// Create a schedule from server-relative values.
    ///
    /// Both targets are deliberately charged from the client's own request
    /// start. Network delay and suspend therefore reduce, rather than expand,
    /// the remaining margin.
    ///
    /// # Errors
    ///
    /// Returns [`ClockError::InvalidSchedule`] for inconsistent relative
    /// durations or [`ClockError::Overflow`] when either target cannot be
    /// represented.
    pub fn from_response(
        request_started: SuspendInstant,
        renew_after_ms: u64,
        expires_in_ms: u64,
    ) -> Result<Self, ClockError> {
        if renew_after_ms > expires_in_ms {
            return Err(ClockError::InvalidSchedule);
        }
        Ok(Self {
            renew_at: request_started.checked_add(Duration::from_millis(renew_after_ms))?,
            expires_at: request_started.checked_add(Duration::from_millis(expires_in_ms))?,
        })
    }

    /// Whether renewal must begin immediately at `now`.
    ///
    /// # Errors
    ///
    /// This currently cannot fail; the result remains fallible so callers can
    /// preserve the clock-failure boundary if schedule representation evolves.
    pub fn renewal_due(self, now: SuspendInstant) -> Result<bool, ClockError> {
        if now < self.renew_at && now < self.expires_at {
            Ok(false)
        } else {
            Ok(true)
        }
    }

    /// Conservative duration until renewal, clamped to zero.
    ///
    /// # Errors
    ///
    /// This currently cannot fail; the result remains fallible alongside the
    /// other schedule operations.
    pub const fn renew_in(self, now: SuspendInstant) -> Result<Duration, ClockError> {
        Ok(self.renew_at.0.saturating_sub(now.0))
    }

    /// Whether the conservative local expiry bound has elapsed.
    #[must_use]
    pub fn expired(self, now: SuspendInstant) -> bool {
        now >= self.expires_at
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "fixed test inputs use expect to keep scheduling assertions focused"
)]
mod tests {
    use super::*;

    #[test]
    fn delayed_response_consumes_renewal_margin() {
        let started = SuspendInstant::from_duration(Duration::from_secs(100));
        let schedule = RenewalSchedule::from_response(started, 10_000, 30_000).expect("schedule");
        assert!(
            !schedule
                .renewal_due(SuspendInstant::from_duration(Duration::from_secs(109)))
                .expect("clock")
        );
        assert!(
            schedule
                .renewal_due(SuspendInstant::from_duration(Duration::from_secs(111)))
                .expect("clock")
        );
        assert!(schedule.expired(SuspendInstant::from_duration(Duration::from_secs(130))));
    }

    #[test]
    fn invalid_or_overflowing_schedule_fails_closed() {
        let started = SuspendInstant::from_duration(Duration::MAX);
        assert!(matches!(
            RenewalSchedule::from_response(started, 20, 10),
            Err(ClockError::InvalidSchedule)
        ));
        assert!(matches!(
            RenewalSchedule::from_response(started, 1, 1),
            Err(ClockError::Overflow)
        ));
    }
}
