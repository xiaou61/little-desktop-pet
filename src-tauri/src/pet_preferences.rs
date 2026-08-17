use std::{
    fs::OpenOptions,
    io::{self, Write},
    os::windows::ffi::OsStrExt,
    path::Path,
};

use serde::{Deserialize, Serialize};
use windows::{
    Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
    core::PCWSTR,
};

pub const DEFAULT_SIZE_PERCENT: u32 = 100;
pub const MIN_SIZE_PERCENT: u32 = 30;
pub const MAX_SIZE_PERCENT: u32 = 160;
pub const SIZE_PERCENT_STEP: u32 = 10;
const PREFERENCES_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PetPreferences {
    version: u32,
    size_percent: u32,
}

pub fn normalize_size_percent(value: i64) -> u32 {
    let Ok(value) = u32::try_from(value) else {
        return DEFAULT_SIZE_PERCENT;
    };
    if is_valid_size_percent(value) {
        value
    } else {
        DEFAULT_SIZE_PERCENT
    }
}

pub fn load_size_percent(path: &Path) -> u32 {
    let Ok(bytes) = std::fs::read(path) else {
        return DEFAULT_SIZE_PERCENT;
    };
    let Ok(preferences) = serde_json::from_slice::<PetPreferences>(&bytes) else {
        return DEFAULT_SIZE_PERCENT;
    };
    if preferences.version != PREFERENCES_VERSION {
        return DEFAULT_SIZE_PERCENT;
    }
    normalize_size_percent(i64::from(preferences.size_percent))
}

pub fn save_size_percent(path: &Path, size_percent: u32) -> io::Result<()> {
    save_size_percent_with(path, size_percent, move_file_replacing)
}

fn save_size_percent_with(
    path: &Path,
    size_percent: u32,
    replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    if !is_valid_size_percent(size_percent) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pet size must be between 30 and 160 in 10 percent steps",
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(&PetPreferences {
        version: PREFERENCES_VERSION,
        size_percent,
    })
    .map_err(io::Error::other)?;

    let result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        replace(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn is_valid_size_percent(value: u32) -> bool {
    (MIN_SIZE_PERCENT..=MAX_SIZE_PERCENT).contains(&value)
        && value.is_multiple_of(SIZE_PERCENT_STEP)
}

fn move_file_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    let source = wide_path(source);
    let destination = wide_path(destination);
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|_| io::Error::last_os_error())
    }
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn size_percent_accepts_only_the_supported_steps() {
        for value in (30..=160).step_by(10) {
            assert_eq!(normalize_size_percent(value), value as u32);
        }
        for invalid in [-500, 0, 20, 29, 31, 99, 159, 161, i64::MAX] {
            assert_eq!(normalize_size_percent(invalid), DEFAULT_SIZE_PERCENT);
        }
    }

    #[test]
    fn missing_corrupt_unknown_and_invalid_preferences_fall_back() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("pet-preferences.json");
        assert_eq!(load_size_percent(&path), DEFAULT_SIZE_PERCENT);

        for contents in [
            "not-json",
            r#"{"version":99,"size_percent":140}"#,
            r#"{"version":1,"size_percent":20}"#,
            r#"{"version":1,"size_percent":105}"#,
            r#"{"version":1,"size_percent":170}"#,
            r#"{"version":1,"size_percent":140,"extra":true}"#,
        ] {
            fs::write(&path, contents).unwrap();
            assert_eq!(load_size_percent(&path), DEFAULT_SIZE_PERCENT);
        }
    }

    #[test]
    fn preferences_round_trip_atomically_in_unicode_windows_paths() {
        let directory = tempdir().unwrap();
        let nested = directory.path().join("桌宠设置");
        let path = nested.join("pet-preferences.json");

        save_size_percent(&path, 150).unwrap();
        assert_eq!(load_size_percent(&path), 150);
        save_size_percent(&path, 80).unwrap();
        assert_eq!(load_size_percent(&path), 80);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"version":1,"size_percent":80}"#
        );
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn invalid_values_are_never_written() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("pet-preferences.json");
        let error = save_size_percent(&path, 105).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!path.exists());
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn failed_replacement_keeps_the_previous_valid_file_and_cleans_the_temporary_file() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("pet-preferences.json");
        save_size_percent(&path, 130).unwrap();

        let error = save_size_percent_with(&path, 150, |_source, _destination| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "simulated replacement failure",
            ))
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(load_size_percent(&path), 130);
        assert!(!path.with_extension("json.tmp").exists());
    }
}
