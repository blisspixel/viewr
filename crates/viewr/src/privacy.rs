//! Fail-closed process privacy boundaries that apply before application threads start.

#[cfg(any(target_os = "linux", test))]
use std::ffi::OsStr;

use crate::Error;

#[cfg(any(target_os = "linux", test))]
fn dbus_address_is_local(value: &OsStr) -> bool {
    let Some(value) = value.to_str() else {
        return false;
    };
    !value.is_empty()
        && value
            .split(';')
            .all(|address| !address.is_empty() && address.starts_with("unix:"))
}

#[cfg(target_os = "linux")]
fn validate_local_dbus_environment() -> Result<(), Error> {
    for name in ["DBUS_SESSION_BUS_ADDRESS", "AT_SPI_BUS_ADDRESS"] {
        if let Some(value) = std::env::var_os(name)
            && !dbus_address_is_local(&value)
        {
            return Err(Error::Platform(
                "Linux privacy boundary requires local Unix D-Bus transports".into(),
            ));
        }
    }
    Ok(())
}

/// Apply platform privacy boundaries before logging, workers, or GUI threads start.
///
/// # Errors
/// Returns [`Error::Platform`] if Linux exposes a non-local D-Bus transport or
/// if the kernel cannot install and verify the Internet-socket seccomp policy.
pub fn apply_startup_hardening() -> Result<(), Error> {
    #[cfg(target_os = "linux")]
    {
        validate_local_dbus_environment()?;
        viewr_seccomp::apply_application_internet_policy().map_err(|error| {
            Error::Platform(format!(
                "Linux privacy boundary could not deny Internet sockets: {error}"
            ))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::dbus_address_is_local;

    #[test]
    fn dbus_transport_validation_accepts_only_unix_domain_addresses() {
        for accepted in [
            "unix:path=/run/user/1000/bus",
            "unix:abstract=/tmp/dbus-address",
            "unix:path=/one;unix:path=/two",
        ] {
            assert!(dbus_address_is_local(OsStr::new(accepted)), "{accepted}");
        }
        for rejected in [
            "",
            "tcp:host=example.invalid,port=1234",
            "nonce-tcp:host=127.0.0.1,port=1234",
            "unixexec:path=/bin/false",
            "ibus:",
            "unix:path=/local;tcp:host=example.invalid,port=1234",
            ";unix:path=/local",
        ] {
            assert!(!dbus_address_is_local(OsStr::new(rejected)), "{rejected}");
        }
    }
}
