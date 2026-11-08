//! The bounded launch: a job that ends the tree, and the child's exit
//! status passed back.

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::{TRUE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, GetExitCodeProcess, ResumeThread, TerminateProcess, WaitForSingleObject,
    CREATE_NO_WINDOW, CREATE_SUSPENDED, INFINITE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOW,
};

use super::{win_error, OwnedHandle, Result};

/// Runs `argv` under `token` and returns the exit code it ended with.
pub fn spawn_bounded(token: &OwnedHandle, argv: &[OsString]) -> Result<u32> {
    let mut command_line = command_line(argv);
    let job = process_tree_job()?;

    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    // CreateProcessAsUser defaults to a noninteractive window station;
    // PowerShell needs the session's interactive desktop.
    startup.lpDesktop = windows_sys::core::w!("winsta0\\default").cast_mut();
    startup.dwFlags = STARTF_USESTDHANDLES;
    startup.hStdInput = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    startup.hStdOutput = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    startup.hStdError = unsafe { GetStdHandle(STD_ERROR_HANDLE) };

    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let flags = CREATE_SUSPENDED | CREATE_NO_WINDOW;
    let ok = unsafe {
        CreateProcessAsUserW(
            token.0,
            std::ptr::null(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            TRUE,
            flags,
            // NULL environment and directory: the child inherits this
            // process's, which the runtime already scrubbed and set to the
            // workspace.
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut info,
        )
    };
    if ok == 0 {
        return Err(win_error("CreateProcessAsUserW"));
    }
    let process = OwnedHandle(info.hProcess);
    let thread = OwnedHandle(info.hThread);

    // The child is suspended, so it cannot run — or spawn — outside the job.
    // If the job will not take it, the boundary cannot be promised: end the
    // child instead of letting it run.
    if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
        let err = win_error("AssignProcessToJobObject");
        unsafe { TerminateProcess(process.0, 1) };
        return Err(err);
    }
    if unsafe { ResumeThread(thread.0) } == u32::MAX {
        return Err(win_error("ResumeThread"));
    }
    if unsafe { WaitForSingleObject(process.0, INFINITE) } != WAIT_OBJECT_0 {
        return Err(win_error("WaitForSingleObject"));
    }
    let mut code = 0u32;
    if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 {
        return Err(win_error("GetExitCodeProcess"));
    }
    Ok(code)
}

/// A job that ends every process in it when the launcher's handle closes —
/// including when the launcher itself is killed.
fn process_tree_job() -> Result<OwnedHandle> {
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw.is_null() {
        return Err(win_error("CreateJobObjectW"));
    }
    let job = OwnedHandle(raw);
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let ok = unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&limits).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if ok == 0 {
        return Err(win_error("SetInformationJobObject"));
    }
    Ok(job)
}

/// The NUL-terminated UTF-16 command line for `CreateProcessAsUserW`, built
/// one quoted argument at a time.
fn command_line(argv: &[OsString]) -> Vec<u16> {
    let mut line: Vec<u16> = Vec::new();
    for (index, arg) in argv.iter().enumerate() {
        if index > 0 {
            line.push(u16::from(b' '));
        }
        push_argument(&mut line, arg);
    }
    line.push(0);
    line
}

/// Appends one argument, quoting it when it is empty or holds whitespace or
/// a quote. Inside the quotes, a run of backslashes is literal except before
/// a quote: the quote takes one extra backslash, and a run at the end of the
/// argument doubles.
fn push_argument(line: &mut Vec<u16>, arg: &OsStr) {
    let units: Vec<u16> = arg.encode_wide().collect();
    let needs_quotes = units.is_empty()
        || units.iter().any(|&unit| {
            unit == u16::from(b' ') || unit == u16::from(b'\t') || unit == u16::from(b'"')
        });
    if !needs_quotes {
        line.extend_from_slice(&units);
        return;
    }

    let backslash = u16::from(b'\\');
    let quote = u16::from(b'"');
    line.push(quote);
    let mut rest = units.as_slice();
    while let Some(&unit) = rest.first() {
        if unit == backslash {
            let run = rest.iter().take_while(|&&unit| unit == backslash).count();
            rest = &rest[run..];
            match rest.first().copied() {
                Some(next) if next == quote => {
                    line.extend(std::iter::repeat_n(backslash, run * 2 + 1));
                    line.push(quote);
                    rest = &rest[1..];
                }
                Some(_) => line.extend(std::iter::repeat_n(backslash, run)),
                None => line.extend(std::iter::repeat_n(backslash, run * 2)),
            }
            continue;
        }
        if unit == quote {
            line.push(backslash);
        }
        line.push(unit);
        rest = &rest[1..];
    }
    line.push(quote);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(argv: &[&str]) -> String {
        let argv: Vec<OsString> = argv.iter().map(OsString::from).collect();
        let mut units = command_line(&argv);
        units.pop(); // the trailing NUL
        String::from_utf16(&units).expect("UTF-16")
    }

    #[test]
    fn plain_arguments_pass_through() {
        assert_eq!(line(&["cmd.exe", "/C", "echo"]), "cmd.exe /C echo");
    }

    #[test]
    fn whitespace_and_empty_arguments_are_quoted() {
        assert_eq!(line(&["a b", ""]), r#""a b" """#);
    }

    #[test]
    fn an_embedded_quote_is_escaped() {
        assert_eq!(line(&[r#"a"b"#]), r#""a\"b""#);
    }

    #[test]
    fn backslashes_before_a_quote_double_plus_one() {
        assert_eq!(line(&[r#"a\"b"#]), r#""a\\\"b""#);
        assert_eq!(line(&[r#"a\\"b"#]), r#""a\\\\\"b""#);
    }

    #[test]
    fn backslashes_away_from_a_quote_stay_literal() {
        assert_eq!(line(&[r"a b\c"]), r#""a b\c""#);
        assert_eq!(line(&[r"a b\\"]), r#""a b\\\\""#);
    }

    #[test]
    fn trailing_backslashes_double_only_before_the_closing_quote() {
        assert_eq!(line(&[r"a b\"]), r#""a b\\""#);
        assert_eq!(line(&[r"a\"]), r"a\");
    }
}
