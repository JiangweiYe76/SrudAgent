//! The platform boundary behind [`wrap_argv`](crate::wrap_argv).
//!
//! A build selects one [`SandboxBackend`](crate::SandboxBackend) here. Linux
//! turns a bounded policy into a bubblewrap command; Windows hands it to a
//! restricted-token launcher; a platform without a backend refuses the policy
//! instead of running it without a boundary.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

use std::ffi::OsString;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use crate::SandboxBackend;
use crate::{BoundedPolicy, SandboxUnavailable, WrappedArgv};

/// The operating system this build targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// The Linux target.
    Linux,
    /// The macOS target.
    MacOs,
    /// The Windows target.
    Windows,
    /// A platform with no runner in this build.
    Other,
}

impl Platform {
    /// The platform this binary was built for.
    #[must_use]
    pub const fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Self::Other
        }
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Linux => "Linux",
            Self::MacOs => "macOS",
            Self::Windows => "Windows",
            Self::Other => "this platform",
        };
        f.write_str(name)
    }
}

/// Turns a bounded policy into the argv this platform's backend executes.
pub(super) fn wrap(
    argv: &[OsString],
    policy: &BoundedPolicy,
) -> Result<WrappedArgv, SandboxUnavailable> {
    #[cfg(target_os = "linux")]
    {
        linux::Bwrap.wrap_argv(argv, policy)
    }
    #[cfg(target_os = "windows")]
    {
        windows::WindowsBoundary.wrap_argv(argv, policy)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (argv, policy);
        Err(SandboxUnavailable::UnsupportedPlatform {
            platform: Platform::current(),
        })
    }
}
