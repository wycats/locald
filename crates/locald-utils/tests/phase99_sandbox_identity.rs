#![allow(missing_docs)]

use std::ffi::OsString;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[allow(unsafe_code)]
fn with_env(vars: &[(&str, Option<&str>)], f: impl FnOnce()) {
    let _guard = match ENV_LOCK.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };

    let mut old: Vec<(String, Option<OsString>)> = Vec::with_capacity(vars.len());
    for (key, _) in vars {
        old.push(((*key).to_string(), std::env::var_os(key)));
    }

    unsafe {
        for (key, value) in vars {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    f();

    unsafe {
        for (key, value) in old {
            match value {
                Some(v) => std::env::set_var(&key, v),
                None => std::env::remove_var(&key),
            }
        }
    }
}

#[test]
fn sandbox_active_without_name_defaults_to_default() {
    with_env(
        &[
            ("LOCALD_SANDBOX_ACTIVE", Some("1")),
            ("LOCALD_SANDBOX_NAME", None),
        ],
        || {
            assert_eq!(
                locald_utils::env::sandbox_name().as_deref(),
                Some("default")
            );
        },
    );
}
