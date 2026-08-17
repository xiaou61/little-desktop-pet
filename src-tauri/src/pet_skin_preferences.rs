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

use crate::pet_skins::{DEFAULT_SKIN_ID, skin_by_id};

const PREFERENCES_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PetSkinPreferences {
    version: u32,
    skin_id: String,
}

pub fn load_skin_id(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return DEFAULT_SKIN_ID.into();
    };
    let Ok(preferences) = serde_json::from_slice::<PetSkinPreferences>(&bytes) else {
        return DEFAULT_SKIN_ID.into();
    };
    if preferences.version != PREFERENCES_VERSION || skin_by_id(&preferences.skin_id).is_none() {
        return DEFAULT_SKIN_ID.into();
    }
    preferences.skin_id
}

pub fn load_plugin_skin_id(path: &Path, is_available: impl Fn(&str) -> bool) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return DEFAULT_SKIN_ID.into();
    };
    let Ok(preferences) = serde_json::from_slice::<PetSkinPreferences>(&bytes) else {
        return DEFAULT_SKIN_ID.into();
    };
    if preferences.version != PREFERENCES_VERSION || !is_available(&preferences.skin_id) {
        return DEFAULT_SKIN_ID.into();
    }
    preferences.skin_id
}

pub fn save_skin_id(path: &Path, skin_id: &str) -> io::Result<()> {
    save_skin_id_with(path, skin_id, move_file_replacing)
}

pub fn save_plugin_skin_id(path: &Path, skin_id: &str) -> io::Result<()> {
    if skin_id.is_empty()
        || skin_id.len() > 96
        || !skin_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown plugin skin ID",
        ));
    }
    save_skin_id_unchecked(path, skin_id, move_file_replacing)
}

fn save_skin_id_with(
    path: &Path,
    skin_id: &str,
    replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    if skin_by_id(skin_id).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unknown pet skin ID",
        ));
    }
    save_skin_id_unchecked(path, skin_id, replace)
}

fn save_skin_id_unchecked(
    path: &Path,
    skin_id: &str,
    replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(&PetSkinPreferences {
        version: PREFERENCES_VERSION,
        skin_id: skin_id.into(),
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
    use std::{fs, io};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn missing_corrupt_unknown_version_and_unknown_skin_fall_back_to_cloud() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("pet-skin-preferences.json");
        assert_eq!(load_skin_id(&path), DEFAULT_SKIN_ID);

        for contents in [
            "not-json",
            r#"{"version":2,"skin_id":"calico-cat"}"#,
            r#"{"version":1,"skin_id":"not-a-skin"}"#,
            r#"{"version":1,"skin_id":"calico-cat","extra":true}"#,
        ] {
            fs::write(&path, contents).unwrap();
            assert_eq!(load_skin_id(&path), DEFAULT_SKIN_ID);
        }
    }

    #[test]
    fn valid_preferences_round_trip_and_unknown_ids_are_never_written() {
        let directory = tempdir().unwrap();
        let path = directory
            .path()
            .join("桌宠设置")
            .join("pet-skin-preferences.json");
        save_skin_id(&path, "calico-cat").unwrap();
        assert_eq!(load_skin_id(&path), "calico-cat");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"version":1,"skin_id":"calico-cat"}"#
        );
        assert!(!path.with_extension("json.tmp").exists());
        assert_eq!(
            save_skin_id(&path, "unknown").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn write_failure_keeps_the_previous_file_and_cleans_the_temporary_file() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("pet-skin-preferences.json");
        save_skin_id(&path, "orange-dragon").unwrap();
        let error = save_skin_id_with(&path, "calico-cat", |_source, _destination| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "simulated failure",
            ))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(load_skin_id(&path), "orange-dragon");
        assert!(!path.with_extension("json.tmp").exists());
    }
}
