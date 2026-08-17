use std::{
    ffi::c_void,
    mem::size_of,
    path::Path,
    ptr,
    time::{Duration, Instant},
};

use chrono::{Local, Utc};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
        System::{
            StationsAndDesktops::{
                CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS,
                GetUserObjectInformationW, OpenInputDesktop, UOI_NAME,
            },
            SystemInformation::GetTickCount64,
            Threading::{
                GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
            },
        },
        UI::{
            Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
            WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
        },
    },
    core::{PCWSTR, PWSTR},
};

use crate::model::{ActivitySnapshot, ApplicationIdentity, Availability};

const MAX_PROCESS_PATH_U16: usize = 32_768;
const MAX_DESKTOP_NAME_U16: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdapterIssue {
    ProcessInaccessible,
    InputUnavailable,
    DesktopUnavailable,
    SecureDesktop,
}

pub struct WindowsAdapter {
    started_at: Instant,
    self_pid: u32,
    cached_foreground: Option<(u32, ApplicationIdentity)>,
}

impl WindowsAdapter {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
            self_pid: unsafe { GetCurrentProcessId() },
            cached_foreground: None,
        }
    }

    pub fn sample(&mut self) -> ActivitySnapshot {
        let local_time = Local::now().fixed_offset();
        let observed_utc = local_time.with_timezone(&Utc);
        let (availability, idle_for) = match input_state() {
            Ok(state) => state,
            Err(issue) => (availability_for_issue(issue), Duration::ZERO),
        };

        let (application, is_self_process) = if availability == Availability::Available {
            self.foreground_application()
        } else {
            (None, false)
        };

        ActivitySnapshot {
            monotonic: self.started_at.elapsed(),
            observed_utc,
            local_time,
            idle_for,
            availability,
            application,
            is_self_process,
        }
    }

    fn foreground_application(&mut self) -> (Option<ApplicationIdentity>, bool) {
        let window = unsafe { GetForegroundWindow() };
        if window.0.is_null() {
            self.cached_foreground = None;
            return (None, false);
        }

        let mut pid = 0_u32;
        if unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) } == 0 || pid == 0 {
            self.cached_foreground = None;
            return (None, false);
        }
        if pid == self.self_pid {
            self.cached_foreground = None;
            return (None, true);
        }
        if let Some((cached_pid, identity)) = self.cached_foreground.as_ref()
            && *cached_pid == pid
        {
            return (Some(identity.clone()), false);
        }

        match resolve_process(pid) {
            Ok(identity) => {
                self.cached_foreground = Some((pid, identity.clone()));
                (Some(identity), false)
            }
            Err(AdapterIssue::ProcessInaccessible) => {
                self.cached_foreground = None;
                (None, false)
            }
            Err(_) => {
                self.cached_foreground = None;
                (None, false)
            }
        }
    }
}

fn input_state() -> Result<(Availability, Duration), AdapterIssue> {
    if !is_default_input_desktop()? {
        return Err(AdapterIssue::SecureDesktop);
    }

    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return Err(AdapterIssue::InputUnavailable);
    }

    let current_tick = unsafe { GetTickCount64() } as u32;
    let idle_ms = current_tick.wrapping_sub(info.dwTime) as u64;
    Ok((Availability::Available, Duration::from_millis(idle_ms)))
}

fn is_default_input_desktop() -> Result<bool, AdapterIssue> {
    let desktop = unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) }
        .map_err(|_| AdapterIssue::DesktopUnavailable)?;
    let mut buffer = [0_u16; MAX_DESKTOP_NAME_U16];
    let mut required_bytes = 0_u32;
    let result = unsafe {
        GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_NAME,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            (buffer.len() * size_of::<u16>()) as u32,
            Some(&mut required_bytes),
        )
    };
    let _ = unsafe { CloseDesktop(desktop) };
    result.map_err(|_| AdapterIssue::DesktopUnavailable)?;

    let end = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    let name = String::from_utf16_lossy(&buffer[..end]);
    Ok(name.eq_ignore_ascii_case("default"))
}

fn resolve_process(pid: u32) -> Result<ApplicationIdentity, AdapterIssue> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|_| AdapterIssue::ProcessInaccessible)?;
    let mut buffer = vec![0_u16; MAX_PROCESS_PATH_U16];
    let mut length = buffer.len() as u32;
    let query_result = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    query_result.map_err(|_| AdapterIssue::ProcessInaccessible)?;

    let path = String::from_utf16_lossy(&buffer[..length as usize]);
    let executable_name = Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or(AdapterIssue::ProcessInaccessible)?
        .to_string();
    let metadata = read_version_metadata(&path);
    let display_name = choose_display_name(
        metadata
            .as_ref()
            .and_then(|value| value.file_description.as_deref()),
        metadata
            .as_ref()
            .and_then(|value| value.product_name.as_deref()),
        &executable_name,
    );

    Ok(ApplicationIdentity {
        identity_key: normalize_executable_path(&path),
        executable_path: path,
        executable_name,
        display_name,
    })
}

#[derive(Debug, Default)]
struct VersionMetadata {
    file_description: Option<String>,
    product_name: Option<String>,
}

fn read_version_metadata(path: &str) -> Option<VersionMetadata> {
    let path_wide = wide_null(path);
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(path_wide.as_ptr()), None) };
    if size == 0 {
        return None;
    }

    let mut block = vec![0_u8; size as usize];
    unsafe {
        GetFileVersionInfoW(
            PCWSTR(path_wide.as_ptr()),
            None,
            size,
            block.as_mut_ptr().cast::<c_void>(),
        )
    }
    .ok()?;

    let translations = query_translation(&block).unwrap_or((0x0409, 0x04b0));
    let prefix = format!(
        "\\StringFileInfo\\{:04x}{:04x}",
        translations.0, translations.1
    );
    Some(VersionMetadata {
        file_description: query_version_string(&block, &format!("{prefix}\\FileDescription")),
        product_name: query_version_string(&block, &format!("{prefix}\\ProductName")),
    })
}

fn query_translation(block: &[u8]) -> Option<(u16, u16)> {
    let mut value = ptr::null_mut::<c_void>();
    let mut length = 0_u32;
    let query = wide_null("\\VarFileInfo\\Translation");
    let found = unsafe {
        VerQueryValueW(
            block.as_ptr().cast::<c_void>(),
            PCWSTR(query.as_ptr()),
            &mut value,
            &mut length,
        )
    };
    if !found.as_bool() || value.is_null() || length < 4 {
        return None;
    }

    let values = unsafe { std::slice::from_raw_parts(value.cast::<u16>(), 2) };
    Some((values[0], values[1]))
}

fn query_version_string(block: &[u8], key: &str) -> Option<String> {
    let mut value = ptr::null_mut::<c_void>();
    let mut length = 0_u32;
    let key = wide_null(key);
    let found = unsafe {
        VerQueryValueW(
            block.as_ptr().cast::<c_void>(),
            PCWSTR(key.as_ptr()),
            &mut value,
            &mut length,
        )
    };
    if !found.as_bool() || value.is_null() || length == 0 {
        return None;
    }

    let values = unsafe { std::slice::from_raw_parts(value.cast::<u16>(), length as usize) };
    let end = values
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(values.len());
    let value = String::from_utf16_lossy(&values[..end]).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn normalize_executable_path(path: &str) -> String {
    let normalized = path.trim().replace('/', "\\");
    let without_extended_prefix = if let Some(rest) = normalized.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = normalized.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        normalized
    };
    without_extended_prefix.to_lowercase()
}

fn choose_display_name(
    file_description: Option<&str>,
    product_name: Option<&str>,
    executable_name: &str,
) -> String {
    file_description
        .and_then(non_empty_trimmed)
        .or_else(|| product_name.and_then(non_empty_trimmed))
        .unwrap_or_else(|| executable_name.to_string())
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn availability_for_issue(issue: AdapterIssue) -> Availability {
    match issue {
        AdapterIssue::SecureDesktop => Availability::LockedOrSecureDesktop,
        AdapterIssue::ProcessInaccessible => Availability::Available,
        AdapterIssue::InputUnavailable | AdapterIssue::DesktopUnavailable => {
            Availability::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_paths_are_normalized_case_insensitively() {
        let cases = [
            (
                "C:/Program Files/App/App.EXE",
                "c:\\program files\\app\\app.exe",
            ),
            (
                "\\\\?\\C:\\Program Files\\App\\App.exe",
                "c:\\program files\\app\\app.exe",
            ),
            (
                "\\\\?\\UNC\\Server\\Share\\App.exe",
                "\\\\server\\share\\app.exe",
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(normalize_executable_path(input), expected);
        }
    }

    #[test]
    fn display_metadata_falls_back_without_guessing() {
        assert_eq!(
            choose_display_name(Some("  Visual Editor  "), Some("Suite"), "editor.exe"),
            "Visual Editor"
        );
        assert_eq!(
            choose_display_name(Some(" "), Some(" Suite "), "editor.exe"),
            "Suite"
        );
        assert_eq!(choose_display_name(None, None, "editor.exe"), "editor.exe");
    }

    #[test]
    fn adapter_errors_map_to_conservative_availability() {
        assert_eq!(
            availability_for_issue(AdapterIssue::SecureDesktop),
            Availability::LockedOrSecureDesktop
        );
        assert_eq!(
            availability_for_issue(AdapterIssue::InputUnavailable),
            Availability::Unavailable
        );
        assert_eq!(
            availability_for_issue(AdapterIssue::DesktopUnavailable),
            Availability::Unavailable
        );
        assert_eq!(
            availability_for_issue(AdapterIssue::ProcessInaccessible),
            Availability::Available
        );
    }
}
