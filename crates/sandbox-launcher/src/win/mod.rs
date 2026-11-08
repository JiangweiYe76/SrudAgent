//! The Win32 boundary: a write-restricted token, DACL grants on the writable
//! roots, and the bounded launch.
//!
//! The token keeps the caller's user, but every write is checked against a
//! small set of restricting SIDs: one per writable root, the session SID, and
//! Everyone. Reads are not filtered by that set, which is why the sandbox
//! crate reports this backend as partial enforcement.

mod grants;
mod launch;
mod sid;
mod token;

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::Security::SID_AND_ATTRIBUTES;

use crate::{Invocation, Mode};

/// A setup step that failed. The launcher exits without running the command.
#[derive(Debug)]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;

/// Names a failed Win32 call with the error code it left behind.
fn win_error(context: &str) -> Error {
    let code = unsafe { GetLastError() };
    Error(format!("{context} (Windows error {code})"))
}

/// A null-terminated UTF-16 copy of `text`, for the W APIs.
fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// A HANDLE that closes itself when the launcher is done with it.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// Prepares the boundary and runs the command inside it, returning the
/// child's exit status. Any setup failure returns before the command starts.
pub fn run(invocation: &Invocation) -> Result<u32> {
    let roots = canonical_roots(&invocation.roots)?;
    let root_sids: Vec<sid::Sid> = match invocation.mode {
        Mode::WorkspaceWrite => roots
            .iter()
            .map(|root| sid::root_sid(root))
            .collect::<Result<_>>()?,
        Mode::ReadOnly => Vec::new(),
    };
    for (root, sid) in roots.iter().zip(&root_sids) {
        grants::allow_writes_under(root, sid)?;
    }

    let own = token::process_token()?;
    let session = sid::session_sid(&own)?;
    let everyone = sid::everyone_sid()?;

    let mut restricting = Vec::with_capacity(root_sids.len() + 2);
    // The order is not significant: a write passes when any restricting SID
    // is granted.
    for sid in std::iter::once(&session)
        .chain(root_sids.iter())
        .chain(std::iter::once(&everyone))
    {
        restricting.push(SID_AND_ATTRIBUTES {
            Sid: sid.as_ptr(),
            Attributes: 0,
        });
    }

    let boundary = token::boundary_token(&own, &restricting)?;
    token::allow_own_objects(&boundary, &session)?;
    launch::spawn_bounded(&boundary, &invocation.inner)
}

/// The roots as canonical absolute paths; a root that is not there fails the
/// setup rather than granting a name that could later point somewhere else.
fn canonical_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    roots
        .iter()
        .map(|root| {
            std::fs::canonicalize(root).map_err(|err| {
                Error(format!(
                    "the writable root `{}` is not usable: {err}",
                    root.display()
                ))
            })
        })
        .collect()
}
