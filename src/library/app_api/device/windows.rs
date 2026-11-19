use super::super::{CACHE, LOCK};
use super::{private_error, random_hex, MAX_CACHE};
use anyhow::Result;
use std::{
    ffi::{c_void, CStr},
    fs::{self, File, OpenOptions},
    io::Write,
    mem::{offset_of, size_of, size_of_val},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr::{addr_of, null, null_mut},
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FileRenameInformation, NtCreateFile, NtSetInformationFile, FILE_CREATE,
            FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
            FILE_RENAME_INFORMATION, FILE_SYNCHRONOUS_IO_NONALERT, FILE_WRITE_THROUGH,
            NTCREATEFILE_CREATE_DISPOSITION,
        },
    },
    Win32::{
        Foundation::{
            LocalFree, ERROR_SUCCESS, GENERIC_EXECUTE, GENERIC_READ, HANDLE, OBJ_CASE_INSENSITIVE,
            STATUS_OBJECT_NAME_NOT_FOUND, UNICODE_STRING,
        },
        Security::{
            Authorization::{
                ConvertSidToStringSidA, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
            },
            EqualSid, GetAce, GetSecurityDescriptorControl, GetTokenInformation, IsWellKnownSid,
            TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid, ACCESS_ALLOWED_ACE,
            ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE,
            OWNER_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
        },
        Storage::FileSystem::{
            FileDispositionInfo, GetFileInformationByHandle, GetFileType,
            GetVolumeInformationByHandleW, SetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
            DELETE, FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
            FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FILE_TRAVERSE, FILE_TYPE_DISK, READ_CONTROL, SYNCHRONIZE,
        },
        System::{
            SystemServices::{
                ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE, FILE_PERSISTENT_ACLS,
            },
            Threading::{GetCurrentProcess, OpenProcessToken},
            IO::IO_STATUS_BLOCK,
        },
    },
};

struct LocalMemory(*mut c_void);

impl Drop for LocalMemory {
    fn drop(&mut self) {
        // LocalFree accepts NULL and owns the allocation returned by the Win32 APIs below.
        unsafe { LocalFree(self.0) };
    }
}

struct UserSid(Vec<usize>);

impl UserSid {
    fn current() -> Result<Self> {
        let mut token = null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(private_error());
        }
        // SAFETY: OpenProcessToken returned an owned, non-inheritable handle.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut bytes = 0;
        // First query obtains the required storage, including the SID pointed to by TOKEN_USER.
        unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut bytes) };
        if (bytes as usize) < size_of::<TOKEN_USER>() {
            return Err(private_error());
        }
        let mut buffer = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
            )
        } == 0
        {
            return Err(private_error());
        }
        Ok(Self(buffer))
    }

    fn as_sid(&self) -> PSID {
        // SAFETY: a successful TokenUser query populated TOKEN_USER and its SID in this buffer.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn private_descriptor(&self) -> Result<LocalMemory> {
        let mut sid_text = null_mut();
        if unsafe { ConvertSidToStringSidA(self.as_sid(), &mut sid_text) } == 0 {
            return Err(private_error());
        }
        let _memory = LocalMemory(sid_text.cast());
        // SAFETY: conversion returned a NUL-terminated SID string owned by _memory.
        let sid = unsafe { CStr::from_ptr(sid_text.cast()) }
            .to_str()
            .map_err(|_| private_error())?;
        // Explicit owner plus a protected, non-inheriting DACL created before any bytes exist.
        descriptor(&format!("O:{sid}D:P(A;;FA;;;{sid})"))
    }
}

fn descriptor(sddl: &str) -> Result<LocalMemory> {
    let sddl: Vec<_> = sddl.encode_utf16().chain([0]).collect();
    let mut descriptor = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(private_error());
    }
    Ok(LocalMemory(descriptor))
}

fn trusted_sid(sid: PSID, user: &UserSid, directory: bool) -> bool {
    // SAFETY: callers supply SIDs from the OS-provided security descriptor.
    unsafe {
        EqualSid(sid, user.as_sid()) != 0
            || (directory
                && (IsWellKnownSid(sid, WinLocalSystemSid) != 0
                    || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0))
    }
}

fn check_acl(file: &File, user: &UserSid, directory: bool) -> Result<()> {
    let mut owner = null_mut();
    let mut acl: *mut ACL = null_mut();
    let mut descriptor = null_mut();
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor,
        )
    };
    let _memory = LocalMemory(descriptor);
    if status != ERROR_SUCCESS || owner.is_null() || acl.is_null() {
        return Err(private_error());
    }
    if !trusted_sid(owner, user, directory) {
        return Err(private_error());
    }
    // SAFETY: GetSecurityInfo returned a non-null ACL pointing into _memory.
    let count = unsafe { (*acl).AceCount };
    if !directory {
        let mut control = 0;
        let mut revision = 0;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
            || control & SE_DACL_PROTECTED == 0
            || count != 1
        {
            return Err(private_error());
        }
    }

    const READ_ONLY: u32 =
        FILE_GENERIC_READ | FILE_GENERIC_EXECUTE | GENERIC_READ | GENERIC_EXECUTE;
    for index in 0..count {
        let mut ace = null_mut();
        if unsafe { GetAce(acl, u32::from(index), &mut ace) } == 0 || ace.is_null() {
            return Err(private_error());
        }
        // SAFETY: GetAce returns a pointer to this ACE's aligned header within the live ACL.
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        let kind = u32::from(header.AceType);
        let flags = u32::from(header.AceFlags);
        if directory && (flags & INHERIT_ONLY_ACE != 0 || kind == ACCESS_DENIED_ACE_TYPE) {
            continue;
        }
        if kind != ACCESS_ALLOWED_ACE_TYPE
            || usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>() + 4
        {
            return Err(private_error());
        }
        // SAFETY: this is a regular allow ACE with a SID of at least its fixed header length.
        let (mask, sid) = unsafe {
            let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
            ((*ace).Mask, addr_of!((*ace).SidStart).cast_mut().cast())
        };
        if directory {
            // The data directory may have inherited read access. Nobody untrusted may
            // modify it or create/delete children; files themselves must be owner-only.
            if mask & !READ_ONLY != 0 && !trusted_sid(sid, user, true) {
                return Err(private_error());
            }
        } else if flags != 0 || mask != FILE_ALL_ACCESS || !trusted_sid(sid, user, false) {
            return Err(private_error());
        }
    }
    Ok(())
}

fn metadata(file: &File, directory: bool) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let handle = file.as_raw_handle();
    if unsafe { GetFileType(handle) } != FILE_TYPE_DISK
        || unsafe { GetFileInformationByHandle(handle, &mut info) } == 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
        || (!directory && info.nNumberOfLinks != 1)
    {
        return Err(private_error());
    }
    Ok(info)
}

fn open_directory(path: &Path, user: &UserSid) -> Result<File> {
    // No delete sharing: don't allow the data directory to be swapped out while in use.
    let file = OpenOptions::new()
        .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES | FILE_TRAVERSE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| private_error())?;
    metadata(&file, true)?;
    let mut fs_flags = 0;
    if unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            &mut fs_flags,
            null_mut(),
            0,
        )
    } == 0
        || fs_flags & FILE_PERSISTENT_ACLS == 0
    {
        return Err(private_error());
    }
    check_acl(&file, user, true)?;
    Ok(file)
}

fn open_child(
    directory: &File,
    name: &str,
    access: u32,
    share: u32,
    disposition: NTCREATEFILE_CREATE_DISPOSITION,
    security: Option<&LocalMemory>,
) -> Result<Option<File>> {
    debug_assert!(!name.contains(['/', '\\', ':', '\0']));
    let mut name: Vec<u16> = name.encode_utf16().collect();
    let length = u16::try_from(size_of_val(name.as_slice())).map_err(|_| private_error())?;
    let name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: directory.as_raw_handle(),
        ObjectName: &name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: security.map_or(null(), |security| security.0.cast()),
        SecurityQualityOfService: null(),
    };
    let mut status_block = IO_STATUS_BLOCK::default();
    let mut handle = null_mut();
    let mut options =
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT;
    if disposition == FILE_CREATE {
        options |= FILE_WRITE_THROUGH;
    }
    // Resolve a single leaf name from the exact directory handle whose ACL we checked.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            access | SYNCHRONIZE,
            &attributes,
            &mut status_block,
            null(),
            FILE_ATTRIBUTE_NORMAL,
            share,
            disposition,
            options,
            null(),
            0,
        )
    };
    if status == STATUS_OBJECT_NAME_NOT_FOUND && disposition == FILE_OPEN {
        return Ok(None);
    }
    if status < 0 || handle.is_null() {
        return Err(private_error());
    }
    // SAFETY: successful NtCreateFile returned a new, non-inheritable file handle.
    Ok(Some(unsafe { File::from_raw_handle(handle) }))
}

pub(super) fn open_lock(path: &Path) -> Result<(File, File)> {
    fs::create_dir_all(path).map_err(|_| private_error())?;
    let user = UserSid::current()?;
    let directory = open_directory(path, &user)?;
    let security = user.private_descriptor()?;
    let file = open_child(
        &directory,
        LOCK,
        FILE_GENERIC_READ | FILE_GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        FILE_OPEN_IF,
        Some(&security),
    )?
    .ok_or_else(private_error)?;
    metadata(&file, false)?;
    check_acl(&file, &user, false)?;
    Ok((directory, file))
}

fn open_existing_cache(directory: &File, user: &UserSid) -> Result<Option<File>> {
    let Some(file) = open_child(
        directory,
        CACHE,
        FILE_GENERIC_READ,
        FILE_SHARE_READ,
        FILE_OPEN,
        None,
    )?
    else {
        return Ok(None);
    };
    let info = metadata(&file, false)?;
    let size = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
    if size > MAX_CACHE {
        return Err(private_error());
    }
    check_acl(&file, user, false)?;
    Ok(Some(file))
}

pub(super) fn open_cache(path: &Path) -> Result<Option<File>> {
    let user = UserSid::current()?;
    let directory = open_directory(path, &user)?;
    open_existing_cache(&directory, &user)
}

fn replace_cache(file: &File) -> Result<()> {
    #[repr(C)]
    struct Rename {
        replace: [u8; 4],
        root: HANDLE,
        length: u32,
        // Include the native header's trailing padding in the supplied buffer size.
        name: [u16; CACHE.len() + 2],
    }
    const _: () =
        assert!(offset_of!(Rename, name) == offset_of!(FILE_RENAME_INFORMATION, FileName));
    let mut rename = Rename {
        replace: [1, 0, 0, 0],
        root: null_mut(),
        length: 0,
        name: [0; CACHE.len() + 2],
    };
    for (dest, source) in rename.name.iter_mut().zip(CACHE.encode_utf16()) {
        *dest = source;
        rename.length += size_of::<u16>() as u32;
    }
    let mut status_block = IO_STATUS_BLOCK::default();
    // A null root and bare leaf rename in the source's own directory. This replaces
    // the directory entry in one operation and keeps the temporary file's private DACL.
    let status = unsafe {
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut status_block,
            addr_of!(rename).cast(),
            size_of::<Rename>() as u32,
            FileRenameInformation,
        )
    };
    if status < 0 {
        return Err(private_error());
    }
    Ok(())
}

pub(super) fn save_cache(path: &Path, bytes: &[u8]) -> Result<()> {
    let user = UserSid::current()?;
    let directory = open_directory(path, &user)?;
    // Never overwrite a pre-existing cache with insecure storage or an unexpected type.
    let existing = open_existing_cache(&directory, &user)?;
    drop(existing);
    let security = user.private_descriptor()?;
    let temporary = format!(".official-app-{}.tmp", random_hex(16));
    let mut file = open_child(
        &directory,
        &temporary,
        FILE_GENERIC_WRITE | DELETE,
        0,
        FILE_CREATE,
        Some(&security),
    )?
    .ok_or_else(private_error)?;
    let result = (|| {
        metadata(&file, false)?;
        check_acl(&file, &user, false)?;
        file.write_all(bytes).map_err(|_| private_error())?;
        file.sync_all().map_err(|_| private_error())?;
        replace_cache(&file)
    })();
    if result.is_err() {
        // Delete the scratch file by handle on close, never by a re-resolved path.
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                addr_of!(disposition).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            );
        }
    }
    result
}

#[cfg(test)]
pub(in crate::library::app_api) fn grant_everyone_full_access(path: &Path) -> Result<()> {
    use windows_sys::Win32::{
        Security::{
            Authorization::SetSecurityInfo, GetSecurityDescriptorDacl,
            PROTECTED_DACL_SECURITY_INFORMATION,
        },
        Storage::FileSystem::WRITE_DAC,
    };

    let descriptor = descriptor("D:P(A;;FA;;;WD)")?;
    let mut acl = null_mut();
    let mut present = 0;
    let mut defaulted = 0;
    if unsafe { GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut acl, &mut defaulted) }
        == 0
        || present == 0
        || acl.is_null()
    {
        return Err(private_error());
    }
    let file = OpenOptions::new()
        .access_mode(WRITE_DAC)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let status = unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(private_error());
    }
    Ok(())
}
