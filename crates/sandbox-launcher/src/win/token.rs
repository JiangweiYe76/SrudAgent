//! The process token and its write-restricted derivative.

use windows_sys::Win32::Foundation::{LocalFree, GENERIC_ALL, HANDLE};
use windows_sys::Win32::Security::Authorization::{SetEntriesInAclW, GRANT_ACCESS};
use windows_sys::Win32::Security::{
    CreateRestrictedToken, SetTokenInformation, TokenDefaultDacl, DISABLE_MAX_PRIVILEGE, LUA_TOKEN,
    NO_INHERITANCE, SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY,
    TOKEN_DEFAULT_DACL, TOKEN_DUPLICATE, TOKEN_QUERY, WRITE_RESTRICTED,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::grants::allow_entry;
use super::{sid, win_error, Error, OwnedHandle, Result};

/// Opens this process's token with the rights the restriction needs.
pub fn process_token() -> Result<OwnedHandle> {
    let access = TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ADJUST_DEFAULT | TOKEN_ASSIGN_PRIMARY;
    let mut raw: HANDLE = std::ptr::null_mut();
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut raw) };
    if ok == 0 {
        return Err(win_error("OpenProcessToken"));
    }
    Ok(OwnedHandle(raw))
}

/// Derives the token the boundary runs under.
///
/// `LUA_TOKEN` makes this a least-privilege token, `DISABLE_MAX_PRIVILEGE`
/// leaves `SeChangeNotifyPrivilege` as the only privilege, and
/// `WRITE_RESTRICTED` makes the system check every write against
/// `restricting` on top of the token's own SIDs. The integrity level is left
/// unchanged; the write restriction is the boundary.
pub fn boundary_token(
    source: &OwnedHandle,
    restricting: &[SID_AND_ATTRIBUTES],
) -> Result<OwnedHandle> {
    let flags = DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED;
    let mut raw: HANDLE = std::ptr::null_mut();
    let ok = unsafe {
        CreateRestrictedToken(
            source.0,
            flags,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            restricting.len() as u32,
            restricting.as_ptr(),
            &mut raw,
        )
    };
    if ok == 0 {
        return Err(win_error("CreateRestrictedToken"));
    }
    Ok(OwnedHandle(raw))
}

/// Names the session SID in the token's default DACL.
///
/// Objects the child creates without an explicit DACL take this one, and the
/// system checks writes to them against the token's restricting SIDs. The
/// session SID is one of them, so naming it here keeps those objects usable.
pub fn allow_own_objects(token: &OwnedHandle, session: &sid::Sid) -> Result<()> {
    let entry = allow_entry(session.as_ptr(), GENERIC_ALL, GRANT_ACCESS, NO_INHERITANCE);
    let mut acl = std::ptr::null_mut();
    let err = unsafe { SetEntriesInAclW(1, &entry, std::ptr::null(), &mut acl) };
    if err != 0 {
        return Err(Error(format!("SetEntriesInAclW (Windows error {err})")));
    }
    let dacl = TOKEN_DEFAULT_DACL { DefaultDacl: acl };
    let ok = unsafe {
        SetTokenInformation(
            token.0,
            TokenDefaultDacl,
            std::ptr::from_ref(&dacl).cast(),
            std::mem::size_of::<TOKEN_DEFAULT_DACL>() as u32,
        )
    };
    unsafe { LocalFree(acl.cast()) };
    if ok == 0 {
        return Err(win_error("SetTokenInformation(TokenDefaultDacl)"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_process_token_carries_a_session_sid() {
        let token = process_token().expect("process token");
        sid::session_sid(&token).expect("session SID");
    }

    #[test]
    fn the_boundary_token_takes_the_default_dacl() {
        let token = process_token().expect("process token");
        let session = sid::session_sid(&token).expect("session SID");
        let everyone = sid::everyone_sid().expect("Everyone SID");
        let restricting = [
            SID_AND_ATTRIBUTES {
                Sid: session.as_ptr(),
                Attributes: 0,
            },
            SID_AND_ATTRIBUTES {
                Sid: everyone.as_ptr(),
                Attributes: 0,
            },
        ];
        let boundary = boundary_token(&token, &restricting).expect("boundary token");
        allow_own_objects(&boundary, &session).expect("default DACL");
    }
}
