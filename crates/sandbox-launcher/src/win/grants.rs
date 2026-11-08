//! The grant that lets a writable root's SID write under it.

use std::path::Path;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, ACCESS_MODE, EXPLICIT_ACCESS_W,
    NO_MULTIPLE_TRUSTEE, SET_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    EqualSid, GetAce, ACCESS_ALLOWED_ACE, ACE_FLAGS, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
};

use super::{sid::Sid, wide, Error, Result};

/// What a writable root grants: read, write, execute, and delete, inherited by
/// the files and directories created under it.
const ROOT_ACCESS: u32 = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;

/// `ACCESS_ALLOWED_ACE_TYPE`; the constant lives in a windows-sys module this
/// crate does not otherwise need.
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

/// Grants write access under `root` to `sid`, unless that exact grant is
/// already there.
pub fn allow_writes_under(root: &Path, sid: &Sid) -> Result<()> {
    let name = wide(root.as_os_str());
    let mut old_dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let err = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut old_dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if err != 0 {
        return Err(Error(format!(
            "reading the ACL on `{}` (Windows error {err})",
            root.display()
        )));
    }
    // The returned DACL points inside the descriptor; one free covers both.
    let result = merge_grant(&name, root, sid, old_dacl);
    unsafe { LocalFree(descriptor.cast()) };
    result
}

/// Writes the merged DACL back, skipping the inheritance pass when nothing
/// changed.
fn merge_grant(name: &[u16], root: &Path, sid: &Sid, old_dacl: *const ACL) -> Result<()> {
    if unsafe { already_granted(old_dacl, sid.as_ptr()) } {
        return Ok(());
    }
    let entry = allow_entry(
        sid.as_ptr(),
        ROOT_ACCESS,
        SET_ACCESS,
        SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    );
    let mut new_dacl: *mut ACL = std::ptr::null_mut();
    let err = unsafe { SetEntriesInAclW(1, &entry, old_dacl, &mut new_dacl) };
    if err != 0 {
        return Err(Error(format!(
            "building the ACL for `{}` (Windows error {err})",
            root.display()
        )));
    }
    let err = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new_dacl,
            std::ptr::null_mut(),
        )
    };
    unsafe { LocalFree(new_dacl.cast()) };
    if err != 0 {
        return Err(Error(format!(
            "granting `{}` (Windows error {err})",
            root.display()
        )));
    }
    Ok(())
}

/// One access entry for a SID trustee.
pub fn allow_entry(
    sid: PSID,
    mask: u32,
    mode: ACCESS_MODE,
    inheritance: ACE_FLAGS,
) -> EXPLICIT_ACCESS_W {
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
        grfAccessMode: mode,
        grfInheritance: inheritance,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            // With TRUSTEE_IS_SID the name field carries the SID pointer.
            ptstrName: sid.cast(),
        },
    }
}

/// Whether the DACL already carries this SID's grant with the same mask and
/// inheritance, so a repeated run leaves it alone.
unsafe fn already_granted(dacl: *const ACL, sid: PSID) -> bool {
    if dacl.is_null() {
        return false;
    }
    let count = unsafe { (*dacl).AceCount };
    for index in 0..u32::from(count) {
        let mut raw: *mut std::ffi::c_void = std::ptr::null_mut();
        if unsafe { GetAce(dacl, index, &mut raw) } == 0 || raw.is_null() {
            return false;
        }
        let header = unsafe { &*(raw as *const ACE_HEADER) };
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE {
            continue;
        }
        let ace = unsafe { &*(raw as *const ACCESS_ALLOWED_ACE) };
        if ace.Header.AceFlags as u32 == SUB_CONTAINERS_AND_OBJECTS_INHERIT
            && ace.Mask == ROOT_ACCESS
            && unsafe { EqualSid(std::ptr::from_ref(&ace.SidStart).cast_mut().cast(), sid) } != 0
        {
            return true;
        }
    }
    false
}
