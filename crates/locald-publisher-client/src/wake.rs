use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use thiserror::Error;

/// Failure to establish or retain wake observation for publisher renewal.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WakeError {
    /// No conforming platform wake monitor is active.
    #[error("publisher wake monitoring is unavailable")]
    Unavailable,
    /// An active monitor failed and can no longer establish wake ordering.
    #[error("publisher wake monitoring failed: {0}")]
    Failed(String),
}

/// Receiver registered by the client-owned lease supervisor.
pub trait WakeSink: Send + Sync + fmt::Debug {
    /// Notify the supervisor that the system resumed.
    fn resumed(&self);
    /// Fail the supervisor closed when wake observation becomes unreliable.
    fn failed(&self, error: WakeError);
}

/// Keeps one wake subscription active until dropped.
pub trait WakeRegistration: Send + fmt::Debug {}

/// Injectable source of system wake events.
///
/// The supported client owns the reaction and renewal ordering. Platform code
/// owns only observation and reports events through the registered sink.
pub trait WakeMonitor: Send + Sync + fmt::Debug {
    /// Register one lease supervisor.
    ///
    /// # Errors
    ///
    /// Returns [`WakeError`] when wake observation cannot be established.
    fn register(&self, sink: Arc<dyn WakeSink>) -> Result<Box<dyn WakeRegistration>, WakeError>;
}

/// Deliberately inactive monitor used while production publisher transport is
/// not advertised. Acquisition fails before a lease can depend on it.
#[derive(Debug, Clone, Copy, Default)]
pub struct InactiveWakeMonitor;

impl WakeMonitor for InactiveWakeMonitor {
    fn register(&self, _sink: Arc<dyn WakeSink>) -> Result<Box<dyn WakeRegistration>, WakeError> {
        Err(WakeError::Unavailable)
    }
}

/// Production wake monitor for supported desktop Unix hosts.
///
/// macOS uses I/O Kit power notifications. Linux uses logind's
/// `PrepareForSleep` signal while retaining a delay inhibitor, releasing the
/// inhibitor only after the sleep transition has been observed and
/// reacquiring it before reporting resume. Other platforms fail closed.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemWakeMonitor;

impl WakeMonitor for SystemWakeMonitor {
    fn register(&self, sink: Arc<dyn WakeSink>) -> Result<Box<dyn WakeRegistration>, WakeError> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            spawn_observer(platform::PlatformObserver, &sink)
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = sink;
            Err(WakeError::Unavailable)
        }
    }
}

#[cfg(any(test, target_os = "linux", target_os = "macos"))]
trait WakeObserver: Send + 'static {
    /// A failed readiness send means startup was cancelled. The observer must
    /// release any platform registration it acquired and return without
    /// delivering events to the sink.
    fn observe(
        self,
        stop: Arc<AtomicBool>,
        sink: Arc<dyn WakeSink>,
        ready: mpsc::SyncSender<Result<(), WakeError>>,
    );
}

#[cfg(any(test, target_os = "linux", target_os = "macos"))]
fn spawn_observer(
    observer: impl WakeObserver,
    sink: &Arc<dyn WakeSink>,
) -> Result<Box<dyn WakeRegistration>, WakeError> {
    const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

    spawn_observer_with_timeout(observer, sink, STARTUP_TIMEOUT)
}

#[cfg(any(test, target_os = "linux", target_os = "macos"))]
fn spawn_observer_with_timeout(
    observer: impl WakeObserver,
    sink: &Arc<dyn WakeSink>,
    startup_timeout: Duration,
) -> Result<Box<dyn WakeRegistration>, WakeError> {
    let stop = Arc::new(AtomicBool::new(false));
    let observer_stop = Arc::clone(&stop);
    let observer_sink = Arc::clone(sink);
    let panic_sink = Arc::clone(sink);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name("locald-publisher-wake".to_owned())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                observer.observe(observer_stop.clone(), observer_sink, ready_tx);
            }));
            if result.is_err() && !observer_stop.load(Ordering::Acquire) {
                notify_failed(
                    &panic_sink,
                    WakeError::Failed("platform wake observer terminated unexpectedly".to_owned()),
                );
            }
        })
        .map_err(|_| WakeError::Failed("could not start platform wake observer".to_owned()))?;

    match ready_rx.recv_timeout(startup_timeout) {
        Ok(Ok(())) => Ok(Box::new(SystemWakeRegistration {
            stop,
            thread: Some(thread),
        })),
        Ok(Err(error)) => {
            stop.store(true, Ordering::Release);
            drop(ready_rx);
            // A failed observer may still be unwinding platform registration.
            // Detach it so failure remains bounded; it owns everything needed
            // to observe `stop` and clean up before exiting.
            drop(thread);
            Err(error)
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            stop.store(true, Ordering::Release);
            drop(ready_rx);
            // Do not join the operation that just exceeded its startup bound.
            // Dropping the readiness receiver fences late success, while the
            // detached observer retains its resources until cleanup completes.
            drop(thread);
            Err(WakeError::Failed(
                "platform wake observer registration timed out".to_owned(),
            ))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            stop.store(true, Ordering::Release);
            drop(ready_rx);
            drop(thread);
            Err(WakeError::Failed(
                "platform wake observer stopped during registration".to_owned(),
            ))
        }
    }
}

#[cfg(any(test, target_os = "linux", target_os = "macos"))]
struct SystemWakeRegistration {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

#[cfg(any(test, target_os = "linux", target_os = "macos"))]
impl fmt::Debug for SystemWakeRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SystemWakeRegistration")
            .field("active", &!self.stop.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

#[cfg(any(test, target_os = "linux", target_os = "macos"))]
impl WakeRegistration for SystemWakeRegistration {}

#[cfg(any(test, target_os = "linux", target_os = "macos"))]
impl Drop for SystemWakeRegistration {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let Some(thread) = self.thread.take() else {
            return;
        };
        if thread.thread().id() != thread::current().id() {
            drop(thread.join());
        }
    }
}

#[cfg(any(test, target_os = "linux", target_os = "macos"))]
fn notify_resumed(sink: &Arc<dyn WakeSink>) -> bool {
    catch_unwind(AssertUnwindSafe(|| sink.resumed())).is_ok()
}

#[cfg(any(test, target_os = "linux", target_os = "macos"))]
fn notify_failed(sink: &Arc<dyn WakeSink>, error: WakeError) {
    drop(catch_unwind(AssertUnwindSafe(|| sink.failed(error))));
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod platform {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, mpsc};

    use super::{WakeError, WakeObserver, WakeSink, notify_failed, notify_resumed};

    type IoObject = u32;
    type IoConnect = IoObject;
    type IoNotificationPort = *mut c_void;
    type CfRunLoop = *mut c_void;
    type CfRunLoopSource = *mut c_void;
    type CfString = *const c_void;

    const IO_OBJECT_NULL: IoObject = 0;
    const K_IO_RETURN_SUCCESS: i32 = 0;
    const K_IO_MESSAGE_CAN_SYSTEM_SLEEP: u32 = 0xe000_0270;
    const K_IO_MESSAGE_SYSTEM_WILL_SLEEP: u32 = 0xe000_0280;
    const K_IO_MESSAGE_SYSTEM_HAS_POWERED_ON: u32 = 0xe000_0300;
    const CF_RUN_LOOP_RUN_FINISHED: i32 = 1;
    const CF_RUN_LOOP_RUN_STOPPED: i32 = 2;
    const RUN_LOOP_SLICE_SECONDS: f64 = 0.1;

    type PowerCallback = unsafe extern "C" fn(*mut c_void, IoObject, u32, *mut c_void);

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IORegisterForSystemPower(
            reference: *mut c_void,
            notification_port: *mut IoNotificationPort,
            callback: PowerCallback,
            notifier: *mut IoObject,
        ) -> IoConnect;
        fn IODeregisterForSystemPower(notifier: *mut IoObject) -> i32;
        fn IOAllowPowerChange(root_port: IoConnect, notification_id: isize) -> i32;
        fn IONotificationPortGetRunLoopSource(
            notification_port: IoNotificationPort,
        ) -> CfRunLoopSource;
        fn IONotificationPortDestroy(notification_port: IoNotificationPort);
        fn IOServiceClose(connection: IoConnect) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: CfString;
        fn CFRunLoopGetCurrent() -> CfRunLoop;
        fn CFRunLoopAddSource(run_loop: CfRunLoop, source: CfRunLoopSource, mode: CfString);
        fn CFRunLoopRemoveSource(run_loop: CfRunLoop, source: CfRunLoopSource, mode: CfString);
        fn CFRunLoopRunInMode(mode: CfString, seconds: f64, return_after_source: bool) -> i32;
    }

    pub(super) struct PlatformObserver;

    struct CallbackState {
        root_port: AtomicU32,
        broken: AtomicBool,
        sink: Arc<dyn WakeSink>,
    }

    impl CallbackState {
        fn fail_once(&self, message: &str) {
            if !self.broken.swap(true, Ordering::AcqRel) {
                notify_failed(&self.sink, WakeError::Failed(message.to_owned()));
            }
        }
    }

    unsafe extern "C" fn power_callback(
        reference: *mut c_void,
        _service: IoObject,
        message_type: u32,
        message_argument: *mut c_void,
    ) {
        if reference.is_null() {
            return;
        }
        // SAFETY: the boxed callback state remains pinned until after the
        // notifier is deregistered and its run-loop source is removed.
        let state = unsafe { &*reference.cast::<CallbackState>() };
        match message_type {
            K_IO_MESSAGE_CAN_SYSTEM_SLEEP | K_IO_MESSAGE_SYSTEM_WILL_SLEEP => {
                let root_port = state.root_port.load(Ordering::Acquire);
                if root_port == IO_OBJECT_NULL {
                    state.fail_once("macOS delivered sleep before wake registration completed");
                    return;
                }
                // SAFETY: this callback received the notification identifier
                // from I/O Kit for this exact registered root-power port.
                let status =
                    unsafe { IOAllowPowerChange(root_port, message_argument.addr().cast_signed()) };
                if status != K_IO_RETURN_SUCCESS {
                    state.fail_once("macOS could not acknowledge a power transition");
                }
            }
            K_IO_MESSAGE_SYSTEM_HAS_POWERED_ON if !notify_resumed(&state.sink) => {
                state.broken.store(true, Ordering::Release);
            }
            _ => {}
        }
    }

    impl WakeObserver for PlatformObserver {
        fn observe(
            self,
            stop: Arc<AtomicBool>,
            sink: Arc<dyn WakeSink>,
            ready: mpsc::SyncSender<Result<(), WakeError>>,
        ) {
            let state = Box::new(CallbackState {
                root_port: AtomicU32::new(IO_OBJECT_NULL),
                broken: AtomicBool::new(false),
                sink: Arc::clone(&sink),
            });
            let state_pointer = Box::into_raw(state);
            let mut notification_port: IoNotificationPort = std::ptr::null_mut();
            let mut notifier = IO_OBJECT_NULL;
            // SAFETY: all out-pointers are writable, the callback has the
            // required ABI, and `state_pointer` remains live through cleanup.
            let root_port = unsafe {
                IORegisterForSystemPower(
                    state_pointer.cast::<c_void>(),
                    &raw mut notification_port,
                    power_callback,
                    &raw mut notifier,
                )
            };
            if root_port == IO_OBJECT_NULL || notification_port.is_null() {
                if !notification_port.is_null() {
                    // SAFETY: the port was returned by I/O Kit even though
                    // complete registration failed.
                    unsafe { IONotificationPortDestroy(notification_port) };
                }
                // SAFETY: no notification source was activated, so no callback
                // can retain or access this allocation.
                drop(unsafe { Box::from_raw(state_pointer) });
                drop(ready.send(Err(WakeError::Unavailable)));
                return;
            }

            // SAFETY: registration succeeded and the callback cannot run until
            // its notification source is attached below.
            unsafe {
                (*state_pointer)
                    .root_port
                    .store(root_port, Ordering::Release);
            };
            // SAFETY: `notification_port` is a live I/O Kit notification port.
            let source = unsafe { IONotificationPortGetRunLoopSource(notification_port) };
            if source.is_null() {
                cleanup_registration(root_port, &mut notifier, notification_port);
                // SAFETY: the source was never activated and registration is
                // now fully torn down.
                drop(unsafe { Box::from_raw(state_pointer) });
                drop(ready.send(Err(WakeError::Unavailable)));
                return;
            }

            // SAFETY: the current thread owns this run loop and the source and
            // default mode are valid for the registration lifetime.
            let run_loop = unsafe { CFRunLoopGetCurrent() };
            // SAFETY: all CoreFoundation objects are live and thread-local here.
            unsafe { CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode) };
            if ready.send(Ok(())).is_err() {
                stop.store(true, Ordering::Release);
            }

            while !stop.load(Ordering::Acquire) {
                // SAFETY: the registered source remains attached to this
                // thread's run loop for the duration of the loop.
                let outcome = unsafe {
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, RUN_LOOP_SLICE_SECONDS, true)
                };
                // SAFETY: callback state remains live until after the loop.
                if unsafe { (*state_pointer).broken.load(Ordering::Acquire) } {
                    break;
                }
                if matches!(outcome, CF_RUN_LOOP_RUN_FINISHED | CF_RUN_LOOP_RUN_STOPPED)
                    && !stop.load(Ordering::Acquire)
                {
                    // SAFETY: callback state remains live through notifier
                    // teardown below.
                    unsafe {
                        (*state_pointer)
                            .fail_once("macOS power-notification run loop stopped unexpectedly");
                    }
                    break;
                }
            }

            // SAFETY: the run-loop source is still attached to this exact loop.
            unsafe {
                CFRunLoopRemoveSource(run_loop, source, kCFRunLoopDefaultMode);
            }
            cleanup_registration(root_port, &mut notifier, notification_port);
            // SAFETY: notifier delivery is disabled and the source removed, so
            // callbacks can no longer access the state.
            drop(unsafe { Box::from_raw(state_pointer) });
        }
    }

    fn cleanup_registration(
        root_port: IoConnect,
        notifier: &mut IoObject,
        notification_port: IoNotificationPort,
    ) {
        // SAFETY: each handle was returned by the successful registration and
        // is released exactly once in the order prescribed by I/O Kit.
        unsafe {
            let _ = IODeregisterForSystemPower(notifier);
            IONotificationPortDestroy(notification_port);
            let _ = IOServiceClose(root_port);
        }
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod platform {
    use std::ffi::{CStr, c_char, c_int, c_void};
    use std::os::fd::{FromRawFd, OwnedFd, RawFd};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};

    use super::{WakeError, WakeObserver, WakeSink, notify_failed, notify_resumed};

    const BUS_WAIT_MICROS: u64 = 100_000;
    const METHOD_TIMEOUT_MICROS: u64 = 2_000_000;
    const SD_BUS_TYPE_BOOLEAN: c_char = b'b' as c_char;
    const SD_BUS_TYPE_STRING: c_char = b's' as c_char;
    const SD_BUS_TYPE_UNIX_FD: c_char = b'h' as c_char;

    type SdBus = c_void;
    type SdBusMessage = c_void;
    type SdBusSlot = c_void;
    type MessageHandler =
        unsafe extern "C" fn(*mut SdBusMessage, *mut c_void, *mut c_void) -> c_int;

    type BusDefaultSystem = unsafe extern "C" fn(*mut *mut SdBus) -> c_int;
    type BusUnref = unsafe extern "C" fn(*mut SdBus) -> *mut SdBus;
    type BusMatchSignal = unsafe extern "C" fn(
        *mut SdBus,
        *mut *mut SdBusSlot,
        *const c_char,
        *const c_char,
        *const c_char,
        *const c_char,
        MessageHandler,
        *mut c_void,
    ) -> c_int;
    type BusSlotUnref = unsafe extern "C" fn(*mut SdBusSlot) -> *mut SdBusSlot;
    type BusProcess = unsafe extern "C" fn(*mut SdBus, *mut *mut SdBusMessage) -> c_int;
    type BusWait = unsafe extern "C" fn(*mut SdBus, u64) -> c_int;
    type MessageNewMethodCall = unsafe extern "C" fn(
        *mut SdBus,
        *mut *mut SdBusMessage,
        *const c_char,
        *const c_char,
        *const c_char,
        *const c_char,
    ) -> c_int;
    type MessageAppendBasic =
        unsafe extern "C" fn(*mut SdBusMessage, c_char, *const c_void) -> c_int;
    type BusCall = unsafe extern "C" fn(
        *mut SdBus,
        *mut SdBusMessage,
        u64,
        *mut c_void,
        *mut *mut SdBusMessage,
    ) -> c_int;
    type MessageReadBasic = unsafe extern "C" fn(*mut SdBusMessage, c_char, *mut c_void) -> c_int;
    type MessageUnref = unsafe extern "C" fn(*mut SdBusMessage) -> *mut SdBusMessage;

    pub(super) struct PlatformObserver;

    struct DynamicLibrary(*mut c_void);

    impl Drop for DynamicLibrary {
        fn drop(&mut self) {
            // SAFETY: this handle came from one successful `dlopen` and every
            // loaded function is already out of use when the owner is dropped.
            unsafe {
                libc::dlclose(self.0);
            }
        }
    }

    struct SystemdApi {
        bus_default_system: BusDefaultSystem,
        bus_unref: BusUnref,
        bus_match_signal: BusMatchSignal,
        bus_slot_unref: BusSlotUnref,
        bus_process: BusProcess,
        bus_wait: BusWait,
        message_new_method_call: MessageNewMethodCall,
        message_append_basic: MessageAppendBasic,
        bus_call: BusCall,
        message_read_basic: MessageReadBasic,
        message_unref: MessageUnref,
        _library: DynamicLibrary,
    }

    impl SystemdApi {
        fn load() -> Result<Self, WakeError> {
            // SAFETY: the library name is a fixed NUL-terminated literal and
            // the resulting handle is retained for every loaded symbol.
            let handle = unsafe {
                libc::dlopen(
                    c"libsystemd.so.0".as_ptr(),
                    libc::RTLD_NOW | libc::RTLD_LOCAL,
                )
            };
            if handle.is_null() {
                return Err(WakeError::Unavailable);
            }
            let library = DynamicLibrary(handle);
            macro_rules! symbol {
                ($name:literal, $ty:ty) => {{
                    load_symbol::<$ty>(library.0, CStr::from_bytes_with_nul($name).expect("symbol"))
                        .ok_or(WakeError::Unavailable)?
                }};
            }
            Ok(Self {
                bus_default_system: symbol!(b"sd_bus_default_system\0", BusDefaultSystem),
                bus_unref: symbol!(b"sd_bus_unref\0", BusUnref),
                bus_match_signal: symbol!(b"sd_bus_match_signal\0", BusMatchSignal),
                bus_slot_unref: symbol!(b"sd_bus_slot_unref\0", BusSlotUnref),
                bus_process: symbol!(b"sd_bus_process\0", BusProcess),
                bus_wait: symbol!(b"sd_bus_wait\0", BusWait),
                message_new_method_call: symbol!(
                    b"sd_bus_message_new_method_call\0",
                    MessageNewMethodCall
                ),
                message_append_basic: symbol!(b"sd_bus_message_append_basic\0", MessageAppendBasic),
                bus_call: symbol!(b"sd_bus_call\0", BusCall),
                message_read_basic: symbol!(b"sd_bus_message_read_basic\0", MessageReadBasic),
                message_unref: symbol!(b"sd_bus_message_unref\0", MessageUnref),
                _library: library,
            })
        }
    }

    fn load_symbol<T: Copy>(handle: *mut c_void, name: &CStr) -> Option<T> {
        // SAFETY: `handle` is live and `name` is NUL terminated. Callers supply
        // the exact ABI type for each fixed libsystemd symbol.
        let pointer = unsafe { libc::dlsym(handle, name.as_ptr()) };
        if pointer.is_null() {
            None
        } else {
            // SAFETY: function pointers and the dlsym result have the same
            // representation on supported Linux targets; the exact function
            // signature is fixed by libsystemd's stable C API.
            Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&pointer) })
        }
    }

    struct BusGuard<'api> {
        api: &'api SystemdApi,
        pointer: *mut SdBus,
    }

    impl Drop for BusGuard<'_> {
        fn drop(&mut self) {
            // SAFETY: the pointer is one owned reference returned by
            // `sd_bus_default_system` and is unreferenced exactly once.
            unsafe {
                (self.api.bus_unref)(self.pointer);
            }
        }
    }

    struct SlotGuard<'api> {
        api: &'api SystemdApi,
        pointer: *mut SdBusSlot,
    }

    impl Drop for SlotGuard<'_> {
        fn drop(&mut self) {
            // SAFETY: this is the one slot reference created for our match.
            unsafe {
                (self.api.bus_slot_unref)(self.pointer);
            }
        }
    }

    struct MessageGuard<'api> {
        api: &'api SystemdApi,
        pointer: *mut SdBusMessage,
    }

    impl Drop for MessageGuard<'_> {
        fn drop(&mut self) {
            // SAFETY: this is one owned sd-bus message reference.
            unsafe {
                (self.api.message_unref)(self.pointer);
            }
        }
    }

    struct SignalState {
        read_basic: MessageReadBasic,
        pending: Option<bool>,
        invalid: bool,
    }

    unsafe extern "C" fn prepare_for_sleep(
        message: *mut SdBusMessage,
        userdata: *mut c_void,
        _error: *mut c_void,
    ) -> c_int {
        if userdata.is_null() || message.is_null() {
            return 0;
        }
        // SAFETY: userdata points to the `SignalState` retained for the match
        // slot's complete lifetime, and sd-bus invokes the callback serially
        // from this observer thread.
        let state = unsafe { &mut *userdata.cast::<SignalState>() };
        let mut entering_sleep: c_int = 0;
        // SAFETY: the message is live for the callback and the output points to
        // a writable C integer, as required for an sd-bus boolean.
        let status = unsafe {
            (state.read_basic)(
                message,
                SD_BUS_TYPE_BOOLEAN,
                (&raw mut entering_sleep).cast::<c_void>(),
            )
        };
        if status <= 0 || state.pending.is_some() {
            state.invalid = true;
        } else {
            state.pending = Some(entering_sleep != 0);
        }
        0
    }

    impl WakeObserver for PlatformObserver {
        fn observe(
            self,
            stop: Arc<AtomicBool>,
            sink: Arc<dyn WakeSink>,
            ready: mpsc::SyncSender<Result<(), WakeError>>,
        ) {
            let api = match SystemdApi::load() {
                Ok(api) => api,
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            };
            let mut bus_pointer = std::ptr::null_mut();
            // SAFETY: `bus_pointer` is writable and receives one owned bus ref.
            if unsafe { (api.bus_default_system)(&raw mut bus_pointer) } < 0
                || bus_pointer.is_null()
            {
                let _ = ready.send(Err(WakeError::Unavailable));
                return;
            }
            let bus = BusGuard {
                api: &api,
                pointer: bus_pointer,
            };
            let mut signal_state = SignalState {
                read_basic: api.message_read_basic,
                pending: None,
                invalid: false,
            };
            let mut slot_pointer = std::ptr::null_mut();
            // SAFETY: all fixed bus names are NUL-terminated, userdata remains
            // pinned on this stack until the match slot is dropped, and the
            // callback has the exact sd-bus ABI.
            let match_status = unsafe {
                (api.bus_match_signal)(
                    bus.pointer,
                    &raw mut slot_pointer,
                    c"org.freedesktop.login1".as_ptr(),
                    c"/org/freedesktop/login1".as_ptr(),
                    c"org.freedesktop.login1.Manager".as_ptr(),
                    c"PrepareForSleep".as_ptr(),
                    prepare_for_sleep,
                    (&raw mut signal_state).cast::<c_void>(),
                )
            };
            if match_status < 0 || slot_pointer.is_null() {
                let _ = ready.send(Err(WakeError::Unavailable));
                return;
            }
            let _slot = SlotGuard {
                api: &api,
                pointer: slot_pointer,
            };
            let mut inhibitor = match acquire_inhibitor(&api, bus.pointer) {
                Ok(inhibitor) => Some(inhibitor),
                Err(_) => {
                    let _ = ready.send(Err(WakeError::Unavailable));
                    return;
                }
            };

            match process_one(&api, bus.pointer, &mut signal_state) {
                Ok(false) if signal_state.pending.is_none() => {}
                Ok(_) => {
                    let _ = ready.send(Err(WakeError::Unavailable));
                    return;
                }
                Err(_) => {
                    let _ = ready.send(Err(WakeError::Unavailable));
                    return;
                }
            }
            if ready.send(Ok(())).is_err() {
                return;
            }

            let mut sleeping = false;
            while !stop.load(Ordering::Acquire) {
                match process_one(&api, bus.pointer, &mut signal_state) {
                    Ok(true) => {
                        if signal_state.invalid {
                            notify_failed(
                                &sink,
                                WakeError::Failed(
                                    "logind delivered a malformed sleep transition".to_owned(),
                                ),
                            );
                            break;
                        }
                        let Some(entering_sleep) = signal_state.pending.take() else {
                            notify_failed(
                                &sink,
                                WakeError::Failed(
                                    "logind omitted its sleep transition state".to_owned(),
                                ),
                            );
                            break;
                        };
                        if entering_sleep {
                            if sleeping || inhibitor.take().is_none() {
                                notify_failed(
                                    &sink,
                                    WakeError::Failed(
                                        "logind sleep transitions arrived out of order".to_owned(),
                                    ),
                                );
                                break;
                            }
                            // Dropping the delay-inhibitor descriptor is the
                            // acknowledgement that this observer has recorded
                            // the transition and is ready for suspend.
                            sleeping = true;
                        } else {
                            if !sleeping || inhibitor.is_some() {
                                notify_failed(
                                    &sink,
                                    WakeError::Failed(
                                        "logind resume arrived without an observed sleep"
                                            .to_owned(),
                                    ),
                                );
                                break;
                            }
                            match acquire_inhibitor(&api, bus.pointer) {
                                Ok(replacement) => inhibitor = Some(replacement),
                                Err(_) => {
                                    notify_failed(
                                        &sink,
                                        WakeError::Failed(
                                            "could not reacquire the logind sleep inhibitor"
                                                .to_owned(),
                                        ),
                                    );
                                    break;
                                }
                            }
                            sleeping = false;
                            if !notify_resumed(&sink) {
                                break;
                            }
                        }
                    }
                    Ok(false) => {
                        // SAFETY: the bus is live, and the bounded timeout lets
                        // registration teardown observe `stop` promptly.
                        let status = unsafe { (api.bus_wait)(bus.pointer, BUS_WAIT_MICROS) };
                        if status < 0 && status != -libc::EINTR {
                            notify_failed(
                                &sink,
                                WakeError::Failed("logind wake observation failed".to_owned()),
                            );
                            break;
                        }
                    }
                    Err(()) => {
                        notify_failed(
                            &sink,
                            WakeError::Failed("logind wake observation failed".to_owned()),
                        );
                        break;
                    }
                }
            }
        }
    }

    fn process_one(api: &SystemdApi, bus: *mut SdBus, state: &mut SignalState) -> Result<bool, ()> {
        state.invalid = false;
        // SAFETY: the bus is live. Passing a null message output asks sd-bus to
        // dispatch through installed callbacks without returning a message.
        let status = unsafe { (api.bus_process)(bus, std::ptr::null_mut()) };
        if status >= 0 {
            Ok(status > 0)
        } else if status == -libc::EINTR {
            Ok(false)
        } else {
            Err(())
        }
    }

    fn acquire_inhibitor(api: &SystemdApi, bus: *mut SdBus) -> Result<OwnedFd, ()> {
        let mut request_pointer = std::ptr::null_mut();
        // SAFETY: all fixed identifiers are NUL-terminated and the message
        // output is writable.
        let status = unsafe {
            (api.message_new_method_call)(
                bus,
                &raw mut request_pointer,
                c"org.freedesktop.login1".as_ptr(),
                c"/org/freedesktop/login1".as_ptr(),
                c"org.freedesktop.login1.Manager".as_ptr(),
                c"Inhibit".as_ptr(),
            )
        };
        if status < 0 || request_pointer.is_null() {
            return Err(());
        }
        let request = MessageGuard {
            api,
            pointer: request_pointer,
        };
        for value in [
            c"sleep",
            c"locald-publisher-client",
            c"preserve published endpoint renewal ordering",
            c"delay",
        ] {
            // SAFETY: each value is a fixed live NUL-terminated string and the
            // request message remains mutable before the call.
            if unsafe {
                (api.message_append_basic)(
                    request.pointer,
                    SD_BUS_TYPE_STRING,
                    value.as_ptr().cast::<c_void>(),
                )
            } < 0
            {
                return Err(());
            }
        }

        let mut reply_pointer = std::ptr::null_mut();
        // SAFETY: request and bus are live, the timeout is bounded, no error
        // object is requested, and the reply output is writable.
        let status = unsafe {
            (api.bus_call)(
                bus,
                request.pointer,
                METHOD_TIMEOUT_MICROS,
                std::ptr::null_mut(),
                &raw mut reply_pointer,
            )
        };
        if status < 0 || reply_pointer.is_null() {
            return Err(());
        }
        let reply = MessageGuard {
            api,
            pointer: reply_pointer,
        };
        let mut borrowed_fd: RawFd = -1;
        // SAFETY: the reply is live and `borrowed_fd` is writable for the
        // UNIX_FD basic type returned by logind's Inhibit method.
        if unsafe {
            (api.message_read_basic)(
                reply.pointer,
                SD_BUS_TYPE_UNIX_FD,
                (&raw mut borrowed_fd).cast::<c_void>(),
            )
        } <= 0
            || borrowed_fd < 0
        {
            return Err(());
        }
        // SAFETY: `borrowed_fd` is valid for the reply lifetime. Duplicating it
        // with CLOEXEC creates the registration-owned inhibitor capability.
        let owned_fd = unsafe { libc::fcntl(borrowed_fd, libc::F_DUPFD_CLOEXEC, 3) };
        if owned_fd < 0 {
            return Err(());
        }
        // SAFETY: `fcntl` returned a fresh owned descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(owned_fd) })
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::panic,
    reason = "scripted wake fixtures use bounded polling and fail immediately when their deterministic script is invalid"
)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Observed {
        Resumed,
        Failed(WakeError),
    }

    #[derive(Debug, Default)]
    struct RecordingSink {
        events: Mutex<Vec<Observed>>,
    }

    impl WakeSink for RecordingSink {
        fn resumed(&self) {
            self.events.lock().expect("events").push(Observed::Resumed);
        }

        fn failed(&self, error: WakeError) {
            self.events
                .lock()
                .expect("events")
                .push(Observed::Failed(error));
        }
    }

    enum ScriptedEvent {
        Resume,
        Fail,
    }

    struct ScriptedObserver {
        events: mpsc::Receiver<ScriptedEvent>,
        stopped: Arc<AtomicBool>,
        startup_error: Option<WakeError>,
    }

    struct StalledStartupObserver {
        entered: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
        completed: mpsc::SyncSender<(bool, bool)>,
    }

    struct StalledFailureObserver {
        entered: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
        completed: mpsc::SyncSender<bool>,
    }

    impl WakeObserver for ScriptedObserver {
        fn observe(
            self,
            stop: Arc<AtomicBool>,
            sink: Arc<dyn WakeSink>,
            ready: mpsc::SyncSender<Result<(), WakeError>>,
        ) {
            if let Some(error) = self.startup_error {
                let _ = ready.send(Err(error));
                self.stopped.store(true, Ordering::Release);
                return;
            }
            if ready.send(Ok(())).is_err() {
                self.stopped.store(true, Ordering::Release);
                return;
            }
            while !stop.load(Ordering::Acquire) {
                match self.events.recv_timeout(Duration::from_millis(10)) {
                    Ok(ScriptedEvent::Resume) => {
                        if !notify_resumed(&sink) {
                            break;
                        }
                    }
                    Ok(ScriptedEvent::Fail) => {
                        notify_failed(
                            &sink,
                            WakeError::Failed("scripted observer failure".to_owned()),
                        );
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            self.stopped.store(true, Ordering::Release);
        }
    }

    impl WakeObserver for StalledStartupObserver {
        fn observe(
            self,
            stop: Arc<AtomicBool>,
            _sink: Arc<dyn WakeSink>,
            ready: mpsc::SyncSender<Result<(), WakeError>>,
        ) {
            let _ = self.entered.send(());
            let _ = self.release.recv();
            let stopped = stop.load(Ordering::Acquire);
            let late_readiness_rejected = ready.send(Ok(())).is_err();
            let _ = self.completed.send((stopped, late_readiness_rejected));
        }
    }

    impl WakeObserver for StalledFailureObserver {
        fn observe(
            self,
            stop: Arc<AtomicBool>,
            _sink: Arc<dyn WakeSink>,
            ready: mpsc::SyncSender<Result<(), WakeError>>,
        ) {
            let _ = ready.send(Err(WakeError::Unavailable));
            let _ = self.entered.send(());
            let _ = self.release.recv();
            let _ = self.completed.send(stop.load(Ordering::Acquire));
        }
    }

    fn wait_until(predicate: impl Fn() -> bool) {
        for _ in 0..100 {
            if predicate() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("condition did not become true");
    }

    #[test]
    fn injected_observer_delivers_resume_and_failure_in_order() {
        let (events_tx, events_rx) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let sink = Arc::new(RecordingSink::default());
        let registration = spawn_observer(
            ScriptedObserver {
                events: events_rx,
                stopped: Arc::clone(&stopped),
                startup_error: None,
            },
            &(Arc::clone(&sink) as Arc<dyn WakeSink>),
        )
        .expect("registration");

        events_tx.send(ScriptedEvent::Resume).expect("resume");
        events_tx.send(ScriptedEvent::Fail).expect("failure");
        wait_until(|| sink.events.lock().expect("events").len() == 2);
        assert_eq!(
            *sink.events.lock().expect("events"),
            vec![
                Observed::Resumed,
                Observed::Failed(WakeError::Failed("scripted observer failure".to_owned())),
            ]
        );

        drop(registration);
        wait_until(|| stopped.load(Ordering::Acquire));
    }

    #[test]
    fn injected_observer_registration_failure_is_returned() {
        let (_events_tx, events_rx) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let sink = Arc::new(RecordingSink::default());
        let result = spawn_observer(
            ScriptedObserver {
                events: events_rx,
                stopped: Arc::clone(&stopped),
                startup_error: Some(WakeError::Unavailable),
            },
            &(sink as Arc<dyn WakeSink>),
        );
        assert!(matches!(result, Err(WakeError::Unavailable)));
        wait_until(|| stopped.load(Ordering::Acquire));
    }

    #[test]
    fn failed_registration_returns_without_joining_a_stalled_observer() {
        const TEST_WAIT_BOUND: Duration = Duration::from_millis(500);

        let (entered_tx, entered_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let (completed_tx, completed_rx) = mpsc::sync_channel(0);
        let (result_tx, result_rx) = mpsc::sync_channel(0);
        let sink: Arc<dyn WakeSink> = Arc::new(RecordingSink::default());
        let caller = thread::spawn(move || {
            let result = spawn_observer(
                StalledFailureObserver {
                    entered: entered_tx,
                    release: release_rx,
                    completed: completed_tx,
                },
                &sink,
            )
            .map(drop);
            let _ = result_tx.send(result);
        });

        entered_rx
            .recv_timeout(TEST_WAIT_BOUND)
            .expect("observer stalls after reporting failure");
        let error = result_rx
            .recv_timeout(TEST_WAIT_BOUND)
            .expect("startup failure returns without joining the observer")
            .expect_err("failed registration must fail");
        assert_eq!(error, WakeError::Unavailable);

        release_tx.send(()).expect("release stalled observer");
        assert!(
            completed_rx
                .recv_timeout(TEST_WAIT_BOUND)
                .expect("failed observer completion cleans up"),
            "failed observer must observe cancellation"
        );
        caller.join().expect("registration caller");
    }

    #[test]
    fn stalled_registration_times_out_without_joining_and_rejects_late_readiness() {
        const TEST_STARTUP_TIMEOUT: Duration = Duration::from_millis(10);
        const TEST_WAIT_BOUND: Duration = Duration::from_millis(500);

        let (entered_tx, entered_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let (completed_tx, completed_rx) = mpsc::sync_channel(0);
        let (result_tx, result_rx) = mpsc::sync_channel(0);
        let sink: Arc<dyn WakeSink> = Arc::new(RecordingSink::default());
        let caller = thread::spawn(move || {
            let result = spawn_observer_with_timeout(
                StalledStartupObserver {
                    entered: entered_tx,
                    release: release_rx,
                    completed: completed_tx,
                },
                &sink,
                TEST_STARTUP_TIMEOUT,
            )
            .map(drop);
            let _ = result_tx.send(result);
        });

        entered_rx
            .recv_timeout(TEST_WAIT_BOUND)
            .expect("observer enters stalled registration");
        let error = result_rx
            .recv_timeout(TEST_WAIT_BOUND)
            .expect("startup timeout returns without joining the observer")
            .expect_err("stalled registration must fail");
        assert_eq!(
            error,
            WakeError::Failed("platform wake observer registration timed out".to_owned())
        );

        release_tx.send(()).expect("release stalled observer");
        let (stopped, late_readiness_rejected) = completed_rx
            .recv_timeout(TEST_WAIT_BOUND)
            .expect("late observer completion cleans up");
        assert!(stopped, "late observer must observe cancellation");
        assert!(
            late_readiness_rejected,
            "late readiness must not publish registration authority"
        );
        caller.join().expect("registration caller");
    }

    #[test]
    fn dropping_registration_stops_injected_observer() {
        let (_events_tx, events_rx) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let sink = Arc::new(RecordingSink::default());
        let registration = spawn_observer(
            ScriptedObserver {
                events: events_rx,
                stopped: Arc::clone(&stopped),
                startup_error: None,
            },
            &(sink as Arc<dyn WakeSink>),
        )
        .expect("registration");
        drop(registration);
        assert!(stopped.load(Ordering::Acquire));
    }

    #[test]
    fn inactive_monitor_remains_fail_closed() {
        let sink = Arc::new(RecordingSink::default());
        assert!(matches!(
            InactiveWakeMonitor.register(sink as Arc<dyn WakeSink>),
            Err(WakeError::Unavailable)
        ));
    }
}
