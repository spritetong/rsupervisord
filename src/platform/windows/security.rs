// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

//! Windows IPC authorization helpers: map a Unix-style octal `mode` onto a
//! file / Named Pipe DACL. This is the OS authorization layer; caller elevation
//! checks remain the application authentication layer.
//!
//! POSIX `ugo` bits → Windows principals:
//! - owner (`0o700`): process user SID
//! - group (`0o070`): BUILTIN\Administrators
//! - other (`0o007`): Everyone (WORLD)
//!   SYSTEM always retains rights so an elevated service can repair the object.

use std::io;
use std::path::Path;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
use windows_sys::Win32::Security::{
    ACL, ACL_REVISION, AddAccessAllowedAce, CreateWellKnownSid, DACL_SECURITY_INFORMATION,
    GetLengthSid, GetTokenInformation, InitializeAcl, InitializeSecurityDescriptor, IsValidAcl,
    PROTECTED_DACL_SECURITY_INFORMATION, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
    SECURITY_MAX_SID_SIZE, SetSecurityDescriptorDacl, TOKEN_QUERY, TOKEN_USER, TokenUser,
    WELL_KNOWN_SID_TYPE, WinBuiltinAdministratorsSid, WinLocalSystemSid, WinWorldSid,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Rights granted to a matching identity class for connect/read/write.
/// Generic bits are mapped by the object's generic mapping at access-check time.
const CLIENT_RIGHTS: u32 =
    windows_sys::Win32::Foundation::GENERIC_READ | windows_sys::Win32::Foundation::GENERIC_WRITE;

/// SECURITY_DESCRIPTOR_REVISION lives in SystemServices; hardcode to avoid an extra feature.
const SD_REVISION: u32 = 1;

/// Returns true when the Win32 error indicates an authorization failure.
pub fn is_access_denied(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(5)   // ERROR_ACCESS_DENIED
        | Some(1314) // ERROR_PRIVILEGE_NOT_HELD
    )
}

/// Returns true when the endpoint exists but the caller is not authorized.
/// Used by the CLI candidate chain to fail closed instead of falling through.
pub fn is_authorization_error(err: &io::Error) -> bool {
    is_access_denied(err)
}

/// Returns true when the endpoint is simply absent (pipe/file not found, refused).
/// These are safe to skip when walking a candidate chain.
pub fn is_not_found(err: &io::Error) -> bool {
    match err.raw_os_error() {
        Some(2)   // ERROR_FILE_NOT_FOUND
        | Some(3) // ERROR_PATH_NOT_FOUND
        | Some(53) // ERROR_BAD_NETPATH
        | Some(10061) // WSAECONNREFUSED
        => true,
        _ => matches!(err.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused),
    }
}

/// Copies the current process user SID out of the process token.
fn current_user_sid() -> io::Result<Vec<u8>> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let _guard = scopeguard::guard(token, |t| {
            CloseHandle(t);
        });

        let mut needed: u32 = 0;
        let _ = GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = vec![0u8; needed as usize];
        if GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        // TOKEN_USER { User: SID_AND_ATTRIBUTES { Sid, Attributes } }; Sid is the first field.
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let sid = user.User.Sid;
        if sid.is_null() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "null user SID"));
        }
        let len = GetLengthSid(sid) as usize;
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(std::slice::from_raw_parts(sid.cast::<u8>(), len).to_vec())
    }
}

/// Materializes a well-known SID into a heap buffer sized to the actual SID.
fn well_known_sid(kind: WELL_KNOWN_SID_TYPE) -> io::Result<Vec<u8>> {
    unsafe {
        let mut sid = vec![0u8; SECURITY_MAX_SID_SIZE as usize];
        let mut size = SECURITY_MAX_SID_SIZE;
        if CreateWellKnownSid(
            kind,
            std::ptr::null_mut(),
            sid.as_mut_ptr().cast(),
            &mut size,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        sid.truncate(size as usize);
        Ok(sid)
    }
}

/// Builds an ASCII DACL granting `CLIENT_RIGHTS` per the `mode` ugo bits.
fn build_dacl(mode: u32) -> io::Result<Vec<u8>> {
    let owner_bits = (mode >> 6) & 0o7;
    let group_bits = (mode >> 3) & 0o7;
    let other_bits = mode & 0o7;

    let mut sids: Vec<Vec<u8>> = Vec::with_capacity(4);
    if owner_bits != 0 {
        sids.push(current_user_sid()?);
    }
    if group_bits != 0 {
        sids.push(well_known_sid(WinBuiltinAdministratorsSid)?);
    }
    if other_bits != 0 {
        sids.push(well_known_sid(WinWorldSid)?);
    }
    // SYSTEM always retains connect rights for elevated service repair.
    sids.push(well_known_sid(WinLocalSystemSid)?);

    // ACL header (8) + per ACE: header+mask (8) + SID, 4-aligned.
    let mut acl_len: usize = 8;
    for sid in &sids {
        let ace_len = 8 + sid.len();
        acl_len += (ace_len + 3) & !3;
    }

    let mut acl_buf = vec![0u8; acl_len];
    unsafe {
        if InitializeAcl(
            acl_buf.as_mut_ptr().cast::<ACL>(),
            acl_len as u32,
            ACL_REVISION,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        for sid in &sids {
            if AddAccessAllowedAce(
                acl_buf.as_mut_ptr().cast::<ACL>(),
                ACL_REVISION,
                CLIENT_RIGHTS,
                sid.as_ptr().cast_mut().cast(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        if IsValidAcl(acl_buf.as_ptr().cast::<ACL>()) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(acl_buf)
}

/// Applies `mode` as the DACL of a filesystem object (Windows AF_UNIX `.sock`).
///
/// Uses `PROTECTED_DACL_SECURITY_INFORMATION` so parent-directory inheritance
/// cannot re-open the socket after bind.
pub fn apply_file_mode(path: &Path, mode: u32) -> io::Result<()> {
    let acl_buf = build_dacl(mode)?;
    let wide = to_wide(path.as_os_str().as_encoded_bytes());
    let info = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
    unsafe {
        let status = SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            info,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl_buf.as_ptr().cast::<ACL>(),
            std::ptr::null_mut(),
        );
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
    }
    Ok(())
}

/// Builds a `SECURITY_DESCRIPTOR` carrying the DACL for `mode`.
/// Returned buffer layout: `[sd 40 bytes][acl ...]` with `sd.Dacl` pointing into `acl`.
pub fn build_security_descriptor(mode: u32) -> io::Result<Vec<u8>> {
    let acl_buf = build_dacl(mode)?;
    // SECURITY_DESCRIPTOR is a fixed-size struct; append ACL after it so one allocation owns both.
    let sd_size = std::mem::size_of::<SECURITY_DESCRIPTOR>();
    let mut blob = vec![0u8; sd_size + acl_buf.len()];
    let (sd_bytes, acl_bytes) = blob.split_at_mut(sd_size);
    acl_bytes.copy_from_slice(&acl_buf);

    unsafe {
        let sd = sd_bytes.as_mut_ptr().cast::<SECURITY_DESCRIPTOR>();
        if InitializeSecurityDescriptor(sd.cast(), SD_REVISION) == 0 {
            return Err(io::Error::last_os_error());
        }
        if SetSecurityDescriptorDacl(sd.cast(), 1, acl_bytes.as_ptr().cast::<ACL>(), 0) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(blob)
}

/// Creates the Named Pipe **anchor** instance with `mode` baked into
/// `SECURITY_ATTRIBUTES` at creation time (zero race). The anchor is held open
/// for the listener lifetime so the pipe object — and its DACL — survive across
/// tokio worker instance create/destroy cycles. Subsequent `CreateNamedPipe`
/// calls ignore their own SD when the name already exists and reuse this one.
///
/// Prefer `tokio::net::windows::named_pipe::ServerOptions::create_with_security_attributes_raw`
/// when creating worker instances; this helper only seeds the first instance.
pub fn create_pipe_anchor(pipe_name: &str, mode: u32) -> io::Result<HANDLE> {
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    let mut sd_blob = build_security_descriptor(mode)?;
    let sd_ptr = sd_blob.as_mut_ptr();
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd_ptr.cast(),
        bInheritHandle: 0,
    };

    let wide = to_wide(pipe_name.as_bytes());
    unsafe {
        let handle = CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            255, // PIPE_UNLIMITED_INSTANCES
            65536,
            65536,
            0,
            &sa,
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // CreateNamedPipeW copies the SECURITY_DESCRIPTOR into the kernel object;
        // sd_blob can be dropped once the call returns.
        Ok(handle)
    }
}

/// Builds a stack-friendly `SECURITY_ATTRIBUTES` pointing at `sd_blob`.
/// Caller must keep `sd_blob` alive for the duration of the Create* call.
pub fn security_attributes_from_blob(sd_blob: &mut [u8]) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd_blob.as_mut_ptr().cast(),
        bInheritHandle: 0,
    }
}

/// Closes a pipe anchor handle (RAII used by the listener).
///
/// # Safety
/// `handle` must be a valid handle previously returned by `CreateNamedPipeW`
/// (or any other Win32 create API) and must not already be closed.
pub unsafe fn close_pipe_anchor(handle: HANDLE) {
    if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
        unsafe {
            CloseHandle(handle);
        }
    }
}

/// Converts bytes to a NUL-terminated UTF-16 buffer (no external wide crate).
fn to_wide(bytes: &[u8]) -> Vec<u16> {
    let s = String::from_utf8_lossy(bytes);
    let mut w: Vec<u16> = s.encode_utf16().collect();
    w.push(0);
    w
}

/// Test-only probe: open the named pipe for read/write as a client would.
#[cfg(test)]
pub fn pipe_client_connect(pipe_name: &str) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
    };
    let wide = to_wide(pipe_name.as_bytes());
    unsafe {
        let h = CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            std::ptr::null_mut(),
        );
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return Err(io::Error::last_os_error());
        }
        CloseHandle(h);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_access_denied() {
        assert!(is_access_denied(&io::Error::from_raw_os_error(5)));
        assert!(is_access_denied(&io::Error::from_raw_os_error(1314)));
        assert!(!is_access_denied(&io::Error::from_raw_os_error(2)));
    }

    #[test]
    fn test_is_not_found() {
        assert!(is_not_found(&io::Error::from_raw_os_error(2)));
        assert!(is_not_found(&io::Error::from_raw_os_error(3)));
        assert!(!is_not_found(&io::Error::from_raw_os_error(5)));
        assert!(is_not_found(&io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "refused"
        )));
    }

    #[test]
    fn test_is_authorization_error_fail_closed() {
        assert!(is_authorization_error(&io::Error::from_raw_os_error(5)));
        assert!(!is_authorization_error(&io::Error::from_raw_os_error(2)));
    }

    #[test]
    fn test_build_dacl_owner_only() {
        // 0700: owner + SYSTEM, no Everyone — DACL must build successfully.
        let acl = build_dacl(0o700).expect("dacl");
        assert!(!acl.is_empty());
    }

    #[test]
    fn test_build_dacl_world() {
        let acl = build_dacl(0o777).expect("dacl");
        assert!(!acl.is_empty());
    }

    #[test]
    fn test_build_dacl_zero_mode_still_has_system() {
        // ugo all zero still retains SYSTEM so elevated repair works.
        let acl = build_dacl(0o000).expect("dacl");
        assert!(!acl.is_empty());
    }

    #[test]
    fn test_build_security_descriptor_blob() {
        let blob = build_security_descriptor(0o700).expect("sd");
        assert!(blob.len() >= std::mem::size_of::<SECURITY_DESCRIPTOR>());
    }

    #[test]
    fn test_to_wide_null_term() {
        let w = to_wide(b"abc");
        assert_eq!(w, vec![97, 98, 99, 0]);
    }
}
