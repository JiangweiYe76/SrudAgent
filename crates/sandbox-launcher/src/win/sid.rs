//! The SIDs a write-restricted token is checked against.

use std::path::Path;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows_sys::Win32::Security::{
    CopySid, CreateWellKnownSid, GetLengthSid, GetTokenInformation, TokenGroups, WinWorldSid, PSID,
    SECURITY_MAX_SID_SIZE, TOKEN_GROUPS,
};

use super::{win_error, Error, OwnedHandle, Result};

/// Marks the logon-session SID in a token's group list. The constant lives in
/// a windows-sys module this crate does not otherwise need; the value is the
/// documented `SE_GROUP_LOGON_ID` attribute.
const SE_GROUP_LOGON_ID: u32 = 0xC000_0000;

/// An owned SID, copied out of whatever buffer the API handed back.
pub struct Sid(Vec<u8>);

impl Sid {
    /// The pointer the token and ACL APIs take.
    pub fn as_ptr(&self) -> PSID {
        self.0.as_ptr() as PSID
    }
}

/// The Everyone SID, `S-1-1-0`.
pub fn everyone_sid() -> Result<Sid> {
    let mut len = SECURITY_MAX_SID_SIZE;
    let mut buf = vec![0u8; len as usize];
    let ok = unsafe {
        CreateWellKnownSid(
            WinWorldSid,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if ok == 0 {
        return Err(win_error("CreateWellKnownSid(WinWorldSid)"));
    }
    buf.truncate(len as usize);
    Ok(Sid(buf))
}

/// The SID that stands for one writable root.
///
/// Derived deterministically from the canonical path — case-folded, then
/// spread over four subauthorities — so one directory keeps one SID and its
/// grant is reused across runs instead of piling up.
pub fn root_sid(root: &Path) -> Result<Sid> {
    let key = root.to_string_lossy().to_lowercase();
    let hash = fnv1a_128(key.as_bytes());
    let text = format!(
        "S-1-5-21-{}-{}-{}-{}",
        (hash >> 96) as u32,
        (hash >> 64) as u32,
        (hash >> 32) as u32,
        hash as u32,
    );
    sid_from_text(&text)
}

/// The logon-session SID from the process token's groups.
pub fn session_sid(token: &OwnedHandle) -> Result<Sid> {
    let mut len = 0u32;
    unsafe { GetTokenInformation(token.0, TokenGroups, std::ptr::null_mut(), 0, &mut len) };
    // The first call only sizes the buffer. Allocate in u64 words so the
    // pointer the group list is read through stays aligned.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    let ok = unsafe {
        GetTokenInformation(token.0, TokenGroups, buf.as_mut_ptr().cast(), len, &mut len)
    };
    if ok == 0 {
        return Err(win_error("GetTokenInformation(TokenGroups)"));
    }
    let groups = unsafe { &*(buf.as_ptr() as *const TOKEN_GROUPS) };
    let entries =
        unsafe { std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize) };
    for entry in entries {
        if entry.Attributes & SE_GROUP_LOGON_ID != 0 {
            return copy_sid(entry.Sid);
        }
    }
    Err(Error("the process token carries no session SID".into()))
}

/// Parses a textual SID and owns a copy of the result.
fn sid_from_text(text: &str) -> Result<Sid> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let mut raw: PSID = std::ptr::null_mut();
    let ok = unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut raw) };
    if ok == 0 {
        return Err(win_error("ConvertStringSidToSidW"));
    }
    let copied = copy_sid(raw);
    unsafe { LocalFree(raw.cast()) };
    copied
}

/// Copies a SID into an owned buffer.
fn copy_sid(sid: PSID) -> Result<Sid> {
    let len = unsafe { GetLengthSid(sid) };
    let mut buf = vec![0u8; len as usize];
    let ok = unsafe { CopySid(len, buf.as_mut_ptr().cast(), sid) };
    if ok == 0 {
        return Err(win_error("CopySid"));
    }
    Ok(Sid(buf))
}

/// FNV-1a over 128 bits: a deterministic, dependency-free spread of the path
/// into the four subauthorities of the root SID.
fn fnv1a_128(bytes: &[u8]) -> u128 {
    let mut hash: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    for &byte in bytes {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013b);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_keeps_its_sid_across_case_spellings() {
        let lower = root_sid(Path::new(r"c:\work\project")).expect("SID");
        let upper = root_sid(Path::new(r"C:\Work\Project")).expect("SID");
        assert_eq!(lower.0, upper.0);
    }

    #[test]
    fn different_roots_get_different_sids() {
        let one = root_sid(Path::new(r"C:\work\one")).expect("SID");
        let two = root_sid(Path::new(r"C:\work\two")).expect("SID");
        assert_ne!(one.0, two.0);
    }

    #[test]
    fn the_everyone_sid_is_well_known() {
        let everyone = everyone_sid().expect("Everyone SID");
        // S-1-1-0: revision 1, one subauthority, world authority, value 0.
        assert_eq!(everyone.0, [1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0]);
    }
}
