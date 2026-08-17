//! Declarative plugin protocol, package validation, persistence and lifecycle.
//!
//! The module deliberately contains no Tauri or Win32 types.  Runtime code only
//! receives validated identifiers and contribution descriptors from here.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    fs::{self, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use image::{ImageFormat, RgbaImage};
use semver::Version;
use serde::{Deserialize, Deserializer, Serialize, de::Error as DeError};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::pet_skins::{self, DEFAULT_SKIN_ID};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const MANIFEST_VERSION: u32 = 1;
pub const HOST_API: &str = "1";
pub const REGISTRY_VERSION: u32 = 1;

pub const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_FILE_COUNT: usize = 256;
pub const MAX_TOTAL_UNCOMPRESSED_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_FILE_UNCOMPRESSED_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_IMAGE_DIMENSION: u32 = 2048;
pub const MAX_THUMBNAIL_DIMENSION: u32 = 512;

const BUILTIN_SKIN_IDS: [&str; 3] = ["simple-cloud", "orange-dragon", "calico-cat"];
const RESERVED_PLUGIN_IDS: [&str; 8] = [
    "core-host",
    "plugin-manager",
    "official-usage",
    "official-quick-panel",
    "official-size-settings",
    "simple-cloud",
    "orange-dragon",
    "calico-cat",
];
const SUPPORTED_PERMISSIONS: [&str; 5] = ["pet", "panel", "settings", "menus", "storage"];
const SUPPORTED_KINDS: [&str; 5] = ["official", "resource", "skin", "panel", "utility"];
const FORBIDDEN_EXTENSIONS: [&str; 10] = [
    "bat", "cmd", "dll", "exe", "js", "ps1", "py", "sh", "ts", "wasm",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginError {
    pub code: String,
    pub message: String,
}

impl PluginError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PluginError {}

impl From<io::Error> for PluginError {
    fn from(error: io::Error) -> Self {
        Self::new("plugin_io_error", format!("插件本地存储失败：{error}"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginSource {
    BuiltIn,
    OfficialDirectory,
    LocalImport,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginState {
    Discovered,
    Installed,
    Enabled,
    Disabled,
    Broken,
    Removed,
}

impl PluginState {
    fn can_enable(&self) -> bool {
        matches!(self, Self::Installed | Self::Disabled | Self::Broken)
    }

    fn can_disable(&self) -> bool {
        matches!(self, Self::Enabled | Self::Broken)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContributionType {
    #[serde(rename = "skins")]
    Skins,
    #[serde(rename = "panelCards")]
    PanelCards,
    #[serde(rename = "settings")]
    Settings,
    #[serde(rename = "menus")]
    Menus,
}

impl ContributionType {
    fn label(&self) -> &'static str {
        match self {
            Self::Skins => "skins",
            Self::PanelCards => "panelCards",
            Self::Settings => "settings",
            Self::Menus => "menus",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginAction {
    pub id: String,
    pub action: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginContribution {
    pub id: String,
    #[serde(rename = "type")]
    pub contribution_type: ContributionType,
    #[serde(default, alias = "resourcePath")]
    pub resource: Option<String>,
    #[serde(default, alias = "thumbnailPath")]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub actions: Vec<PluginAction>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManifest {
    pub manifest_version: u32,
    pub id: String,
    pub version: String,
    #[serde(alias = "name")]
    pub display_name: String,
    pub kind: String,
    pub host_api: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(deserialize_with = "deserialize_contributes")]
    pub contributes: Vec<PluginContribution>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawContributes {
    List(Vec<PluginContribution>),
    Groups(BTreeMap<String, Vec<PluginContribution>>),
}

fn deserialize_contributes<'de, D>(deserializer: D) -> Result<Vec<PluginContribution>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = RawContributes::deserialize(deserializer)?;
    match raw {
        RawContributes::List(items) => Ok(items),
        RawContributes::Groups(groups) => {
            let mut contributions = Vec::new();
            for (kind, items) in groups {
                let expected = match kind.as_str() {
                    "skins" => ContributionType::Skins,
                    "panelCards" => ContributionType::PanelCards,
                    "settings" => ContributionType::Settings,
                    "menus" => ContributionType::Menus,
                    _ => return Err(D::Error::custom(format!("未知的插件贡献类型：{kind}"))),
                };
                for item in items {
                    if item.contribution_type != expected {
                        return Err(D::Error::custom("插件贡献分组与贡献 type 不一致"));
                    }
                    contributions.push(item);
                }
            }
            Ok(contributions)
        }
    }
}

impl PluginManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, PluginError> {
        let manifest = serde_json::from_slice::<Self>(bytes).map_err(|error| {
            PluginError::new(
                "plugin_manifest_invalid",
                format!("插件 manifest 无法解析：{error}"),
            )
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), PluginError> {
        if self.manifest_version != MANIFEST_VERSION {
            return Err(PluginError::new(
                "plugin_manifest_version_unsupported",
                "插件 manifest 版本不受当前宿主支持。",
            ));
        }
        validate_id(&self.id, "plugin_manifest_invalid")?;
        if self.display_name.trim().is_empty() || self.display_name.chars().count() > 120 {
            return Err(PluginError::new(
                "plugin_manifest_invalid",
                "插件显示名称不能为空或过长。",
            ));
        }
        if !SUPPORTED_KINDS.contains(&self.kind.as_str()) {
            return Err(PluginError::new(
                "plugin_manifest_kind_unsupported",
                "插件类型不受当前宿主支持。",
            ));
        }
        Version::parse(&self.version).map_err(|_| {
            PluginError::new(
                "plugin_manifest_version_invalid",
                "插件版本必须是语义化版本。",
            )
        })?;
        validate_host_api(&self.host_api)?;

        let mut permissions = HashSet::new();
        for permission in &self.permissions {
            if !SUPPORTED_PERMISSIONS.contains(&permission.as_str()) {
                return Err(PluginError::new(
                    "plugin_permission_unsupported",
                    format!("插件请求了不受支持的权限：{permission}"),
                ));
            }
            if !permissions.insert(permission) {
                return Err(PluginError::new(
                    "plugin_manifest_duplicate_permission",
                    "插件权限声明不能重复。",
                ));
            }
        }

        if self.contributes.is_empty() {
            return Err(PluginError::new(
                "plugin_manifest_invalid",
                "插件至少需要声明一项贡献。",
            ));
        }
        let mut contribution_ids = HashSet::new();
        for contribution in &self.contributes {
            validate_id(&contribution.id, "plugin_contribution_invalid")?;
            if !contribution.id.starts_with(&format!("{}.", self.id)) {
                return Err(PluginError::new(
                    "plugin_contribution_namespace",
                    format!("贡献 ID 必须位于插件 {} 的命名空间内。", self.id),
                ));
            }
            if !contribution_ids.insert(&contribution.id) {
                return Err(PluginError::new(
                    "plugin_contribution_duplicate",
                    "插件贡献 ID 不能重复。",
                ));
            }
            if matches!(contribution.contribution_type, ContributionType::Skins) {
                let resource = contribution.resource.as_deref().ok_or_else(|| {
                    PluginError::new("plugin_contribution_invalid", "皮肤贡献缺少主资源路径。")
                })?;
                let thumbnail = contribution.thumbnail.as_deref().ok_or_else(|| {
                    PluginError::new("plugin_contribution_invalid", "皮肤贡献缺少缩略图路径。")
                })?;
                validate_package_path(resource)?;
                validate_package_path(thumbnail)?;
                if contribution.width.unwrap_or(0) == 0 || contribution.height.unwrap_or(0) == 0 {
                    return Err(PluginError::new(
                        "plugin_contribution_invalid",
                        "皮肤贡献必须声明正数基准尺寸。",
                    ));
                }
            }
            for action in &contribution.actions {
                validate_id(&action.id, "plugin_action_invalid")?;
                if !action.id.starts_with(&format!("{}.", self.id)) {
                    return Err(PluginError::new(
                        "plugin_action_namespace",
                        "固定动作 ID 必须位于插件命名空间内。",
                    ));
                }
                if action.action.trim().is_empty()
                    || action.action.contains(['/', '\\', ':'])
                    || action.action.contains("http")
                {
                    return Err(PluginError::new(
                        "plugin_action_unsupported",
                        "动作只能引用宿主批准的固定动作。",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn contribution_types(&self) -> Vec<ContributionType> {
        let mut values = Vec::new();
        for contribution in &self.contributes {
            if !values.contains(&contribution.contribution_type) {
                values.push(contribution.contribution_type.clone());
            }
        }
        values
    }
}

fn validate_id(id: &str, code: &str) -> Result<(), PluginError> {
    if id.is_empty()
        || id.len() > 96
        || id.starts_with('.')
        || id.ends_with('.')
        || id.contains("..")
        || id.starts_with('-')
        || id.ends_with('-')
        || id.contains("--")
        || !id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
        })
    {
        return Err(PluginError::new(
            code,
            "标识必须使用小写字母、数字、点和连字符。",
        ));
    }
    Ok(())
}

fn validate_host_api(host_api: &str) -> Result<(), PluginError> {
    let major = host_api
        .trim_start_matches(['^', '~', '>', '<', '='])
        .split(['.', ' ', ','])
        .next()
        .and_then(|value| value.parse::<u32>().ok());
    if major != Some(1) {
        return Err(PluginError::new(
            "plugin_host_api_unsupported",
            "插件声明的宿主协议版本不兼容。",
        ));
    }
    Ok(())
}

pub fn validate_package_path(value: &str) -> Result<(), PluginError> {
    if value.is_empty()
        || value.contains('\\')
        || value.contains(':')
        || value.starts_with('/')
        || value.starts_with("//")
    {
        return Err(PluginError::new(
            "plugin_path_invalid",
            "插件资源路径必须是包内相对路径。",
        ));
    }
    let path = Path::new(value);
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(PluginError::new(
                    "plugin_path_invalid",
                    "插件资源路径不能包含绝对路径或路径穿越。",
                ));
            }
        }
    }
    if Path::new(value)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            FORBIDDEN_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
    {
        return Err(PluginError::new(
            "plugin_script_forbidden",
            "插件包不能包含可执行或脚本入口。",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedPackage {
    pub manifest: PluginManifest,
    pub files: BTreeMap<String, Vec<u8>>,
    pub checksum: String,
}

pub fn validate_petpack(bytes: &[u8]) -> Result<ValidatedPackage, PluginError> {
    if bytes.len() as u64 > MAX_PACKAGE_BYTES {
        return Err(PluginError::new(
            "plugin_package_too_large",
            "插件包压缩大小超过限制。",
        ));
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| {
        PluginError::new(
            "plugin_package_invalid",
            format!("插件包不是有效的 ZIP：{error}"),
        )
    })?;
    let mut files = BTreeMap::new();
    let mut names = HashSet::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            PluginError::new(
                "plugin_package_invalid",
                format!("读取插件包条目失败：{error}"),
            )
        })?;
        let name = entry.name().to_string();
        if entry.is_dir() {
            continue;
        }
        validate_package_path(&name)?;
        let key = name.to_ascii_lowercase();
        if !names.insert(key) {
            return Err(PluginError::new(
                "plugin_package_duplicate_entry",
                "插件包包含重复资源路径。",
            ));
        }
        if entry.size() > MAX_FILE_UNCOMPRESSED_BYTES {
            return Err(PluginError::new(
                "plugin_resource_too_large",
                "插件包单个文件超过限制。",
            ));
        }
        total = total.saturating_add(entry.size());
        if total > MAX_TOTAL_UNCOMPRESSED_BYTES {
            return Err(PluginError::new(
                "plugin_package_expanded_too_large",
                "插件包展开总量超过限制。",
            ));
        }
        let mut content = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut content).map_err(|error| {
            PluginError::new(
                "plugin_package_invalid",
                format!("读取插件资源失败：{error}"),
            )
        })?;
        files.insert(name, content);
        if files.len() > MAX_FILE_COUNT {
            return Err(PluginError::new(
                "plugin_package_too_many_files",
                "插件包文件数量超过限制。",
            ));
        }
    }
    let manifest_bytes = files.get(MANIFEST_FILE).ok_or_else(|| {
        PluginError::new(
            "plugin_manifest_missing",
            "插件包根目录缺少 manifest.json。",
        )
    })?;
    let manifest = PluginManifest::parse(manifest_bytes)?;
    for contribution in &manifest.contributes {
        if matches!(contribution.contribution_type, ContributionType::Skins) {
            let resource_path = contribution.resource.as_deref().unwrap_or_default();
            let thumbnail_path = contribution.thumbnail.as_deref().unwrap_or_default();
            let resource = files.get(resource_path).ok_or_else(|| {
                PluginError::new("plugin_resource_missing", "皮肤主资源不在插件包内。")
            })?;
            let thumbnail = files.get(thumbnail_path).ok_or_else(|| {
                PluginError::new("plugin_resource_missing", "皮肤缩略图不在插件包内。")
            })?;
            validate_png_resource(resource, false)?;
            validate_png_resource(thumbnail, true)?;
        }
    }
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let checksum = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(ValidatedPackage {
        manifest,
        files,
        checksum,
    })
}

fn is_reserved_plugin_id(id: &str) -> bool {
    RESERVED_PLUGIN_IDS.contains(&id)
}

fn validate_png_resource(bytes: &[u8], thumbnail: bool) -> Result<RgbaImage, PluginError> {
    let image = image::load_from_memory_with_format(bytes, ImageFormat::Png)
        .map_err(|error| {
            PluginError::new("plugin_image_invalid", format!("PNG 资源无法解码：{error}"))
        })?
        .into_rgba8();
    let max = if thumbnail {
        MAX_THUMBNAIL_DIMENSION
    } else {
        MAX_IMAGE_DIMENSION
    };
    if image.width() == 0 || image.height() == 0 || image.width() > max || image.height() > max {
        return Err(PluginError::new(
            "plugin_image_size_invalid",
            "PNG 资源尺寸超过限制。",
        ));
    }
    validate_transparent_bounds(&image)?;
    Ok(image)
}

fn validate_transparent_bounds(image: &RgbaImage) -> Result<(), PluginError> {
    let corners = [
        image.get_pixel(0, 0)[3],
        image.get_pixel(image.width() - 1, 0)[3],
        image.get_pixel(0, image.height() - 1)[3],
        image.get_pixel(image.width() - 1, image.height() - 1)[3],
    ];
    if corners.iter().any(|alpha| *alpha != 0) {
        return Err(PluginError::new(
            "plugin_image_transparency_invalid",
            "PNG 资源四角必须透明。",
        ));
    }
    if !image.pixels().any(|pixel| pixel[3] > 0) {
        return Err(PluginError::new(
            "plugin_image_transparency_invalid",
            "PNG 资源没有可见像素。",
        ));
    }
    if !image.pixels().any(|pixel| pixel[3] == 0) {
        return Err(PluginError::new(
            "plugin_image_transparency_invalid",
            "PNG 资源必须保留透明边界。",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRecord {
    pub id: String,
    pub version: String,
    pub source: PluginSource,
    pub checksum: Option<String>,
    pub state: PluginState,
    pub permissions: Vec<String>,
    pub last_error: Option<String>,
    pub protected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRegistryFile {
    pub version: u32,
    pub plugins: Vec<PluginRecord>,
}

impl Default for PluginRegistryFile {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            plugins: Vec::new(),
        }
    }
}

pub fn load_registry(path: &Path) -> PluginRegistryFile {
    let Ok(bytes) = fs::read(path) else {
        return PluginRegistryFile::default();
    };
    let Ok(registry) = serde_json::from_slice::<PluginRegistryFile>(&bytes) else {
        return PluginRegistryFile::default();
    };
    if registry.version != REGISTRY_VERSION || !valid_registry_records(&registry.plugins) {
        return PluginRegistryFile::default();
    }
    registry
}

fn valid_registry_records(records: &[PluginRecord]) -> bool {
    let mut ids = HashSet::new();
    records.iter().all(|record| {
        validate_id(&record.id, "plugin_registry_invalid").is_ok()
            && Version::parse(&record.version).is_ok()
            && ids.insert(&record.id)
    })
}

pub fn save_registry(path: &Path, registry: &PluginRegistryFile) -> Result<(), PluginError> {
    save_registry_with(path, registry, replace_file)
}

fn save_registry_with(
    path: &Path,
    registry: &PluginRegistryFile,
    replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<(), PluginError> {
    if registry.version != REGISTRY_VERSION || !valid_registry_records(&registry.plugins) {
        return Err(PluginError::new(
            "plugin_registry_invalid",
            "插件注册表版本或记录无效。",
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(registry).map_err(|error| {
        PluginError::new(
            "plugin_registry_invalid",
            format!("序列化插件注册表失败：{error}"),
        )
    })?;
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
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(PluginError::from)
}

fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::{
            Win32::Storage::FileSystem::{
                MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
            },
            core::PCWSTR,
        };
        let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination_wide: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        unsafe {
            MoveFileExW(
                PCWSTR(source_wide.as_ptr()),
                PCWSTR(destination_wide.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
            .map_err(|_| io::Error::last_os_error())
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, destination)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSummary {
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub kind: String,
    pub source: PluginSource,
    pub state: PluginState,
    pub permissions: Vec<String>,
    pub contributions: Vec<ContributionType>,
    pub last_error: Option<String>,
    pub protected: bool,
    pub installed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCatalogEntry {
    pub id: String,
    pub display_name: String,
    pub version: String,
    pub kind: String,
    pub source: PluginSource,
    pub thumbnail_data_url: Option<String>,
    pub contributions: Vec<ContributionType>,
    pub permissions: Vec<String>,
    pub installed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OfficialCatalogDefinition {
    id: String,
    display_name: String,
    version: String,
    kind: String,
    thumbnail: String,
    contributions: Vec<ContributionType>,
    permissions: Vec<String>,
    source: PluginSource,
}

const OFFICIAL_CATALOG_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/plugins/official-catalog.json"
));

fn official_catalog() -> &'static [OfficialCatalogDefinition] {
    static CATALOG: OnceLock<Vec<OfficialCatalogDefinition>> = OnceLock::new();
    CATALOG
        .get_or_init(|| serde_json::from_str(OFFICIAL_CATALOG_JSON).unwrap_or_default())
        .as_slice()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDirectory {
    pub installed: Vec<PluginSummary>,
    pub available: Vec<PluginCatalogEntry>,
}

#[derive(Default)]
struct ContributionState {
    by_plugin: HashMap<String, Vec<PluginContribution>>,
}

#[derive(Clone)]
pub struct ContributionRegistry {
    inner: Arc<Mutex<ContributionState>>,
}

impl Default for ContributionRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ContributionState::default())),
        }
    }
}

pub struct ContributionHandle {
    registry: Weak<Mutex<ContributionState>>,
    plugin_id: String,
    active: bool,
}

impl ContributionRegistry {
    pub fn register(
        &self,
        plugin_id: &str,
        contributions: &[PluginContribution],
    ) -> Result<ContributionHandle, PluginError> {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let occupied = state
            .by_plugin
            .values()
            .flatten()
            .map(|contribution| contribution.id.as_str())
            .collect::<HashSet<_>>();
        if contributions
            .iter()
            .any(|contribution| occupied.contains(contribution.id.as_str()))
        {
            return Err(PluginError::new(
                "plugin_contribution_conflict",
                "插件贡献 ID 已被其他已启用插件占用。",
            ));
        }
        state
            .by_plugin
            .insert(plugin_id.to_string(), contributions.to_vec());
        Ok(ContributionHandle {
            registry: Arc::downgrade(&self.inner),
            plugin_id: plugin_id.to_string(),
            active: true,
        })
    }

    pub fn list(&self) -> Vec<PluginContribution> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .by_plugin
            .values()
            .flatten()
            .cloned()
            .collect()
    }

    pub fn list_for_plugin(&self, plugin_id: &str) -> Vec<PluginContribution> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .by_plugin
            .get(plugin_id)
            .cloned()
            .unwrap_or_default()
    }

    fn revoke(&self, plugin_id: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .by_plugin
            .remove(plugin_id);
    }
}

impl ContributionHandle {
    pub fn revoke(mut self) {
        self.revoke_inner();
    }

    fn revoke_inner(&mut self) {
        if self.active {
            if let Some(registry) = self.registry.upgrade() {
                registry
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .by_plugin
                    .remove(&self.plugin_id);
            }
            self.active = false;
        }
    }
}

impl Drop for ContributionHandle {
    fn drop(&mut self) {
        self.revoke_inner();
    }
}

#[derive(Default)]
pub struct PluginRuntime {
    registry: ContributionRegistry,
    active: HashMap<String, ContributionHandle>,
}

impl PluginRuntime {
    pub fn contributions(&self) -> ContributionRegistry {
        self.registry.clone()
    }

    pub fn enable(&mut self, manifest: &PluginManifest) -> Result<(), PluginError> {
        if self.active.contains_key(&manifest.id) {
            return Ok(());
        }
        manifest.validate()?;
        let handle = self
            .registry
            .register(&manifest.id, &manifest.contributes)?;
        self.active.insert(manifest.id.clone(), handle);
        Ok(())
    }

    pub fn disable(&mut self, plugin_id: &str) {
        self.active.remove(plugin_id);
    }

    pub fn is_enabled(&self, plugin_id: &str) -> bool {
        self.active.contains_key(plugin_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCapability {
    pub id: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCapabilities {
    pub host_api: String,
    pub capabilities: Vec<HostCapability>,
}

impl Default for HostCapabilities {
    fn default() -> Self {
        Self {
            host_api: HOST_API.into(),
            capabilities: [
                "pet",
                "panel",
                "settings",
                "menus",
                "storage",
                "permissions",
            ]
            .into_iter()
            .map(|id| HostCapability {
                id: id.into(),
                version: "1".into(),
            })
            .collect(),
        }
    }
}

#[derive(Clone, Debug)]
struct InstalledPackage {
    manifest: PluginManifest,
    files: BTreeMap<String, Vec<u8>>,
    checksum: String,
}

struct PluginManagerState {
    root: PathBuf,
    registry_path: PathBuf,
    registry: PluginRegistryFile,
    runtime: PluginRuntime,
    packages: HashMap<String, InstalledPackage>,
}

#[derive(Clone)]
pub struct PluginManager {
    state: Arc<Mutex<PluginManagerState>>,
}

impl PluginManager {
    pub fn bootstrap(root: PathBuf) -> Result<Self, PluginError> {
        fs::create_dir_all(root.join("plugins"))?;
        fs::create_dir_all(root.join("plugin-data"))?;
        let registry_path = root.join("plugin-registry.json");
        let mut registry = load_registry(&registry_path);
        ensure_bootstrap_records(&mut registry);
        save_registry(&registry_path, &registry)?;
        let manager = Self {
            state: Arc::new(Mutex::new(PluginManagerState {
                root,
                registry_path,
                registry,
                runtime: PluginRuntime::default(),
                packages: HashMap::new(),
            })),
        };
        {
            let mut state = manager
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            load_installed_packages(&mut state);
        }
        manager.restore_enabled_records();
        Ok(manager)
    }

    pub fn from_registry_for_test(root: PathBuf) -> Result<Self, PluginError> {
        Self::bootstrap(root)
    }

    pub fn in_memory(root: PathBuf) -> Self {
        let registry_path = root.join("plugin-registry.json");
        let mut registry = PluginRegistryFile::default();
        ensure_bootstrap_records(&mut registry);
        let manager = Self {
            state: Arc::new(Mutex::new(PluginManagerState {
                root,
                registry_path,
                registry,
                runtime: PluginRuntime::default(),
                packages: HashMap::new(),
            })),
        };
        manager.restore_enabled_records();
        manager
    }

    pub fn summaries(&self) -> Vec<PluginSummary> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .registry
            .plugins
            .iter()
            .map(|record| {
                let (display_name, kind, contributions) =
                    if let Some(package) = state.packages.get(&record.id) {
                        (
                            package.manifest.display_name.clone(),
                            package.manifest.kind.clone(),
                            package.manifest.contribution_types(),
                        )
                    } else {
                        builtin_metadata(&record.id)
                    };
                PluginSummary {
                    id: record.id.clone(),
                    display_name,
                    version: record.version.clone(),
                    kind,
                    source: record.source.clone(),
                    state: record.state.clone(),
                    permissions: record.permissions.clone(),
                    contributions,
                    last_error: record.last_error.clone(),
                    protected: record.protected,
                    installed: !matches!(record.state, PluginState::Removed),
                }
            })
            .collect()
    }

    pub fn catalog(&self) -> Vec<PluginCatalogEntry> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        official_catalog()
            .iter()
            .filter_map(|entry| {
                let id = entry.id.as_str();
                let installed =
                    state.registry.plugins.iter().any(|record| {
                        record.id == id && !matches!(record.state, PluginState::Removed)
                    });
                let definition = pet_skins::skin_by_id(id)?;
                let expected_thumbnail = format!("skins/{id}-thumb.png");
                if entry.thumbnail != expected_thumbnail {
                    return None;
                }
                Some(PluginCatalogEntry {
                    id: entry.id.clone(),
                    display_name: entry.display_name.clone(),
                    version: entry.version.clone(),
                    kind: entry.kind.clone(),
                    source: entry.source.clone(),
                    thumbnail_data_url: Some(pet_skins::thumbnail_data_url(definition)),
                    contributions: entry.contributions.clone(),
                    permissions: entry.permissions.clone(),
                    installed,
                })
            })
            .collect()
    }

    pub fn directory(&self) -> PluginDirectory {
        PluginDirectory {
            installed: self.summaries(),
            available: self.catalog(),
        }
    }

    pub fn capabilities(&self) -> HostCapabilities {
        HostCapabilities::default()
    }

    pub fn enabled_skins(&self) -> Vec<PluginCatalogEntry> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .registry
            .plugins
            .iter()
            .filter(|record| matches!(record.state, PluginState::Enabled))
            .filter_map(|record| {
                let package = state.packages.get(&record.id);
                let (display_name, kind, contributions, thumbnail_data_url) =
                    if let Some(package) = package {
                        let contribution =
                            package.manifest.contributes.iter().find(|contribution| {
                                matches!(contribution.contribution_type, ContributionType::Skins)
                            })?;
                        let thumbnail = package
                            .files
                            .get(contribution.thumbnail.as_deref().unwrap_or_default())
                            .map(|bytes| format!("data:image/png;base64,{}", encode_base64(bytes)));
                        (
                            package.manifest.display_name.clone(),
                            package.manifest.kind.clone(),
                            package.manifest.contribution_types(),
                            thumbnail,
                        )
                    } else {
                        let skin = pet_skins::skin_by_id(&record.id)?;
                        (
                            skin.display_name.into(),
                            "skin".into(),
                            vec![ContributionType::Skins],
                            Some(pet_skins::thumbnail_data_url(skin)),
                        )
                    };
                Some(PluginCatalogEntry {
                    id: record.id.clone(),
                    display_name,
                    version: record.version.clone(),
                    kind,
                    source: record.source.clone(),
                    thumbnail_data_url,
                    contributions,
                    permissions: record.permissions.clone(),
                    installed: true,
                })
            })
            .collect()
    }

    pub fn preview_package(&self, bytes: &[u8]) -> Result<PluginSummary, PluginError> {
        let package = validate_petpack(bytes)?;
        let manifest = package.manifest;
        let contributions = manifest.contribution_types();
        Ok(PluginSummary {
            id: manifest.id,
            display_name: manifest.display_name,
            version: manifest.version,
            kind: manifest.kind,
            source: PluginSource::LocalImport,
            state: PluginState::Discovered,
            permissions: manifest.permissions,
            contributions,
            last_error: None,
            protected: false,
            installed: false,
        })
    }

    pub fn install_official_skin(&self, id: &str) -> Result<PluginSummary, PluginError> {
        if !BUILTIN_SKIN_IDS.contains(&id) {
            return Err(PluginError::new("plugin_not_found", "未找到该官方插件。"));
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pet_skins::skin_by_id(id)
            .ok_or_else(|| PluginError::new("plugin_resource_missing", "官方皮肤资源不可用。"))?;
        if let Some(existing) = state
            .registry
            .plugins
            .iter()
            .find(|record| record.id == id && !matches!(record.state, PluginState::Removed))
            .cloned()
        {
            return Ok(summary_for_record(&state, existing));
        }
        let replacement = PluginRecord {
            id: id.into(),
            version: "1.0.0".into(),
            source: if id == DEFAULT_SKIN_ID {
                PluginSource::BuiltIn
            } else {
                PluginSource::OfficialDirectory
            },
            checksum: None,
            state: PluginState::Installed,
            permissions: Vec::new(),
            last_error: None,
            protected: id == DEFAULT_SKIN_ID,
        };
        let index = upsert_record(&mut state.registry, replacement);
        save_registry(&state.registry_path, &state.registry)?;
        Ok(summary_for_record(
            &state,
            state.registry.plugins[index].clone(),
        ))
    }

    pub fn install_package(&self, bytes: &[u8]) -> Result<PluginSummary, PluginError> {
        let package = validate_petpack(bytes)?;
        let id = package.manifest.id.clone();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if is_reserved_plugin_id(&id) {
            return Err(PluginError::new(
                "plugin_id_reserved",
                "该插件标识由桌宠核心或官方目录保留。",
            ));
        }
        let previous_record = state
            .registry
            .plugins
            .iter()
            .find(|record| record.id == id)
            .cloned();
        if previous_record
            .as_ref()
            .is_some_and(|record| matches!(record.state, PluginState::Enabled))
        {
            return Err(PluginError::new(
                "plugin_disable_required",
                "请先禁用现有插件版本，再安装新版本。",
            ));
        }
        let plugin_dir = state
            .root
            .join("plugins")
            .join(&id)
            .join(&package.manifest.version);
        let previous_dir = state
            .root
            .join("plugins")
            .join(&id)
            .join(format!(".{}.previous", package.manifest.version));
        let temporary = state.root.join("plugins").join(format!(".{id}.installing"));
        if temporary.exists() {
            let _ = fs::remove_dir_all(&temporary);
        }
        fs::create_dir_all(&temporary)?;
        let write_result = write_package_files(&temporary, &package.files);
        if let Err(error) = write_result {
            let _ = fs::remove_dir_all(&temporary);
            return Err(error);
        }
        if let Some(parent) = plugin_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        if previous_dir.exists() {
            let _ = fs::remove_dir_all(&previous_dir);
        }
        if plugin_dir.exists() {
            if let Err(error) = fs::rename(&plugin_dir, &previous_dir) {
                let _ = fs::remove_dir_all(&temporary);
                return Err(PluginError::from(error));
            }
        }
        if let Err(error) = replace_directory(&temporary, &plugin_dir) {
            let _ = fs::remove_dir_all(&temporary);
            if previous_dir.exists() {
                let _ = fs::rename(&previous_dir, &plugin_dir);
            }
            return Err(PluginError::from(error));
        }
        let previous_package = state.packages.get(&id).cloned();
        let index = upsert_record(
            &mut state.registry,
            PluginRecord {
                id: id.clone(),
                version: package.manifest.version.clone(),
                source: PluginSource::LocalImport,
                checksum: Some(package.checksum.clone()),
                state: PluginState::Installed,
                permissions: package.manifest.permissions.clone(),
                last_error: None,
                protected: false,
            },
        );
        state.packages.insert(
            id.clone(),
            InstalledPackage {
                manifest: package.manifest,
                files: package.files,
                checksum: package.checksum,
            },
        );
        if let Err(error) = save_registry(&state.registry_path, &state.registry) {
            let _ = fs::remove_dir_all(&plugin_dir);
            if previous_dir.exists() {
                let _ = fs::rename(&previous_dir, &plugin_dir);
            }
            if let Some(previous_record) = previous_record {
                state.registry.plugins[index] = previous_record;
            } else {
                state.registry.plugins.remove(index);
            }
            if let Some(previous_package) = previous_package {
                state.packages.insert(id.clone(), previous_package);
            } else {
                state.packages.remove(&id);
            }
            return Err(error);
        }
        let _ = fs::remove_dir_all(&previous_dir);
        Ok(summary_for_record(
            &state,
            state.registry.plugins[index].clone(),
        ))
    }

    pub fn import_package_file(&self, path: &Path) -> Result<PluginSummary, PluginError> {
        validate_local_import_path(path)?;
        let bytes = fs::read(path).map_err(PluginError::from)?;
        self.install_package(&bytes)
    }

    pub fn enable(&self, id: &str) -> Result<PluginSummary, PluginError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let index = find_record_index(&state.registry, id)?;
        let current = state.registry.plugins[index].clone();
        if matches!(current.state, PluginState::Enabled) {
            return Ok(summary_for_record(&state, current));
        }
        if !current.state.can_enable() {
            return Err(PluginError::new(
                "plugin_state_invalid",
                "当前插件状态不能启用。",
            ));
        }
        let manifest = if let Some(package) = state.packages.get(id) {
            package.manifest.clone()
        } else {
            builtin_manifest(id)
        };
        if let Err(error) = state.runtime.enable(&manifest) {
            let record = &mut state.registry.plugins[index];
            record.state = PluginState::Broken;
            record.last_error = Some(error.message.clone());
            save_registry(&state.registry_path, &state.registry)?;
            return Err(error);
        }
        state.registry.plugins[index].state = PluginState::Enabled;
        state.registry.plugins[index].last_error = None;
        if let Err(error) = save_registry(&state.registry_path, &state.registry) {
            state.runtime.disable(id);
            state.registry.plugins[index] = current;
            return Err(error);
        }
        Ok(summary_for_record(
            &state,
            state.registry.plugins[index].clone(),
        ))
    }

    pub fn disable(&self, id: &str) -> Result<PluginSummary, PluginError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let index = find_record_index(&state.registry, id)?;
        let current = state.registry.plugins[index].clone();
        if matches!(current.state, PluginState::Disabled) {
            return Ok(summary_for_record(&state, current));
        }
        if !current.state.can_disable() {
            return Err(PluginError::new(
                "plugin_state_invalid",
                "当前插件状态不能禁用。",
            ));
        }
        if id == DEFAULT_SKIN_ID && enabled_skin_count(&state) <= 1 {
            return Err(PluginError::new(
                "plugin_default_skin_required",
                "至少需要保留一套可用默认皮肤。",
            ));
        }
        let manifest = if let Some(package) = state.packages.get(id) {
            package.manifest.clone()
        } else {
            builtin_manifest(id)
        };
        state.runtime.disable(id);
        state.registry.plugins[index].state = PluginState::Disabled;
        if let Err(error) = save_registry(&state.registry_path, &state.registry) {
            state.registry.plugins[index] = current;
            let _ = state.runtime.enable(&manifest);
            return Err(error);
        }
        Ok(summary_for_record(
            &state,
            state.registry.plugins[index].clone(),
        ))
    }

    pub fn uninstall(&self, id: &str) -> Result<(), PluginError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let index = find_record_index(&state.registry, id)?;
        let current = state.registry.plugins[index].clone();
        if current.protected || id == DEFAULT_SKIN_ID {
            return Err(PluginError::new(
                "plugin_protected",
                "核心宿主、插件管理器和最后一套默认皮肤不能卸载。",
            ));
        }
        if matches!(current.state, PluginState::Enabled) {
            return Err(PluginError::new(
                "plugin_disable_required",
                "请先禁用插件再卸载。",
            ));
        }
        let plugin_dir = state.root.join("plugins").join(id);
        let temporary = state
            .root
            .join("plugins")
            .join(format!(".{id}.uninstalling"));
        if temporary.exists() {
            fs::remove_dir_all(&temporary)?;
        }
        if plugin_dir.exists() {
            fs::rename(&plugin_dir, &temporary)?;
        }
        state.runtime.disable(id);
        state.registry.plugins[index].state = PluginState::Removed;
        if let Err(error) = save_registry(&state.registry_path, &state.registry) {
            state.registry.plugins[index] = current.clone();
            if temporary.exists() {
                let _ = fs::rename(&temporary, &plugin_dir);
            }
            return Err(error);
        }
        state.packages.remove(id);
        let _ = fs::remove_dir_all(&temporary);
        Ok(())
    }

    pub fn contributions(&self, id: &str) -> Result<Vec<PluginContribution>, PluginError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let record = state
            .registry
            .plugins
            .iter()
            .find(|record| record.id == id)
            .ok_or_else(|| PluginError::new("plugin_not_found", "未找到该插件。"))?;
        if !matches!(record.state, PluginState::Enabled) {
            return Ok(Vec::new());
        }
        Ok(state.runtime.registry.list_for_plugin(id))
    }

    pub fn execute_action(
        &self,
        plugin_id: &str,
        contribution_id: &str,
        action_id: &str,
    ) -> Result<(), PluginError> {
        validate_id(plugin_id, "plugin_id_invalid")?;
        validate_id(contribution_id, "plugin_contribution_invalid")?;
        validate_id(action_id, "plugin_action_invalid")?;
        let contributions = self.contributions(plugin_id)?;
        let contribution = contributions
            .iter()
            .find(|contribution| contribution.id == contribution_id)
            .ok_or_else(|| PluginError::new("plugin_contribution_not_found", "未找到该贡献。"))?;
        if contribution
            .actions
            .iter()
            .any(|action| action.id == action_id)
        {
            Ok(())
        } else {
            Err(PluginError::new(
                "plugin_action_not_found",
                "该动作不是宿主批准的固定动作。",
            ))
        }
    }

    pub fn read_settings(&self, id: &str) -> Result<serde_json::Value, PluginError> {
        validate_id(id, "plugin_id_invalid")?;
        let path = self.settings_path(id);
        let Ok(bytes) = fs::read(path) else {
            return Ok(serde_json::json!({}));
        };
        serde_json::from_slice(&bytes).map_err(|error| {
            PluginError::new(
                "plugin_settings_invalid",
                format!("插件设置无法解析：{error}"),
            )
        })
    }

    pub fn write_settings(&self, id: &str, value: &serde_json::Value) -> Result<(), PluginError> {
        validate_id(id, "plugin_id_invalid")?;
        if !value.is_object() {
            return Err(PluginError::new(
                "plugin_settings_invalid",
                "插件设置必须是 JSON 对象。",
            ));
        }
        let path = self.settings_path(id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(value).map_err(|error| {
            PluginError::new(
                "plugin_settings_invalid",
                format!("插件设置序列化失败：{error}"),
            )
        })?;
        let result = (|| {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            replace_file(&temporary, &path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(PluginError::from)
    }

    pub fn is_skin_enabled(&self, id: &str) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .registry
            .plugins
            .iter()
            .any(|record| record.id == id && matches!(record.state, PluginState::Enabled))
    }

    pub fn skin_png(&self, id: &str) -> Result<Vec<u8>, PluginError> {
        validate_id(id, "plugin_id_invalid")?;
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let record = state
            .registry
            .plugins
            .iter()
            .find(|record| record.id == id)
            .ok_or_else(|| PluginError::new("plugin_not_found", "未找到该皮肤插件。"))?;
        if !matches!(record.state, PluginState::Enabled) {
            return Err(PluginError::new("plugin_not_enabled", "皮肤插件尚未启用。"));
        }
        if let Some(definition) = pet_skins::skin_by_id(id) {
            return Ok(definition.png.to_vec());
        }
        let package = state
            .packages
            .get(id)
            .ok_or_else(|| PluginError::new("plugin_resource_missing", "皮肤插件资源不可用。"))?;
        let contribution = package
            .manifest
            .contributes
            .iter()
            .find(|contribution| matches!(contribution.contribution_type, ContributionType::Skins))
            .ok_or_else(|| PluginError::new("plugin_contribution_invalid", "插件没有皮肤贡献。"))?;
        package
            .files
            .get(contribution.resource.as_deref().unwrap_or_default())
            .cloned()
            .ok_or_else(|| PluginError::new("plugin_resource_missing", "皮肤主资源不可用。"))
    }

    fn settings_path(&self, id: &str) -> PathBuf {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .root
            .join("plugin-data")
            .join(id)
            .join("settings.json")
    }

    fn restore_enabled_records(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let enabled = state
            .registry
            .plugins
            .iter()
            .filter(|record| matches!(record.state, PluginState::Enabled))
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        for id in enabled {
            let manifest = if let Some(package) = state.packages.get(&id) {
                package.manifest.clone()
            } else {
                builtin_manifest(&id)
            };
            if let Err(error) = state.runtime.enable(&manifest) {
                if let Some(record) = state
                    .registry
                    .plugins
                    .iter_mut()
                    .find(|record| record.id == id)
                {
                    record.state = PluginState::Broken;
                    record.last_error = Some(error.message);
                }
            }
        }
        let _ = save_registry(&state.registry_path, &state.registry);
    }
}

fn load_installed_packages(state: &mut PluginManagerState) {
    let records = state
        .registry
        .plugins
        .iter()
        .filter(|record| matches!(record.source, PluginSource::LocalImport))
        .cloned()
        .collect::<Vec<_>>();
    for record in records {
        let package_root = state
            .root
            .join("plugins")
            .join(&record.id)
            .join(&record.version);
        let result = (|| {
            let manifest_bytes = fs::read(package_root.join(MANIFEST_FILE))
                .map_err(|error| PluginError::new("plugin_manifest_missing", error.to_string()))?;
            let manifest = PluginManifest::parse(&manifest_bytes)?;
            if manifest.id != record.id || manifest.version != record.version {
                return Err(PluginError::new(
                    "plugin_registry_mismatch",
                    "插件注册表与安装目录不匹配。",
                ));
            }
            let files = read_package_directory(&package_root)?;
            Ok(InstalledPackage {
                manifest,
                files,
                checksum: record.checksum.clone().unwrap_or_default(),
            })
        })();
        match result {
            Ok(package) => {
                state.packages.insert(record.id.clone(), package);
            }
            Err(error) => {
                if let Some(existing) = state
                    .registry
                    .plugins
                    .iter_mut()
                    .find(|existing| existing.id == record.id)
                {
                    existing.state = PluginState::Broken;
                    existing.last_error = Some(error.message);
                }
            }
        }
    }
}

fn read_package_directory(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, PluginError> {
    fn visit(
        root: &Path,
        current: &Path,
        files: &mut BTreeMap<String, Vec<u8>>,
        names: &mut HashSet<String>,
        total: &mut u64,
    ) -> Result<(), PluginError> {
        for entry in fs::read_dir(current).map_err(PluginError::from)? {
            let entry = entry.map_err(PluginError::from)?;
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, files, names, total)?;
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|error| PluginError::new("plugin_path_invalid", error.to_string()))?;
            let name = relative
                .to_str()
                .ok_or_else(|| {
                    PluginError::new("plugin_path_invalid", "插件资源路径不是有效文本。")
                })?
                .replace('\\', "/");
            validate_package_path(&name)?;
            if files.len() >= MAX_FILE_COUNT {
                return Err(PluginError::new(
                    "plugin_package_too_many_files",
                    "插件包文件数量超过限制。",
                ));
            }
            let metadata = fs::metadata(&path).map_err(PluginError::from)?;
            if metadata.len() > MAX_FILE_UNCOMPRESSED_BYTES {
                return Err(PluginError::new(
                    "plugin_resource_too_large",
                    "插件包单个文件超过限制。",
                ));
            }
            *total = total.saturating_add(metadata.len());
            if *total > MAX_TOTAL_UNCOMPRESSED_BYTES {
                return Err(PluginError::new(
                    "plugin_package_expanded_too_large",
                    "插件包展开总量超过限制。",
                ));
            }
            if !names.insert(name.to_ascii_lowercase()) {
                return Err(PluginError::new(
                    "plugin_package_duplicate_entry",
                    "插件包包含重复资源路径。",
                ));
            }
            files.insert(name, fs::read(path).map_err(PluginError::from)?);
        }
        Ok(())
    }

    let mut files = BTreeMap::new();
    let mut names = HashSet::new();
    let mut total = 0_u64;
    visit(root, root, &mut files, &mut names, &mut total)?;
    let manifest = files.get(MANIFEST_FILE).ok_or_else(|| {
        PluginError::new("plugin_manifest_missing", "安装目录缺少 manifest.json。")
    })?;
    let parsed = PluginManifest::parse(manifest)?;
    for contribution in &parsed.contributes {
        if matches!(contribution.contribution_type, ContributionType::Skins) {
            let resource = files
                .get(contribution.resource.as_deref().unwrap_or_default())
                .ok_or_else(|| {
                    PluginError::new("plugin_resource_missing", "安装目录缺少皮肤主资源。")
                })?;
            let thumbnail = files
                .get(contribution.thumbnail.as_deref().unwrap_or_default())
                .ok_or_else(|| {
                    PluginError::new("plugin_resource_missing", "安装目录缺少皮肤缩略图。")
                })?;
            validate_png_resource(resource, false)?;
            validate_png_resource(thumbnail, true)?;
        }
    }
    Ok(files)
}

fn ensure_bootstrap_records(registry: &mut PluginRegistryFile) {
    let records = [
        ("core-host", "核心宿主", "official", true),
        ("plugin-manager", "插件管理", "official", true),
        ("official-usage", "使用统计", "official", true),
        ("official-quick-panel", "快捷面板", "official", true),
        ("official-size-settings", "大小设置", "official", true),
    ];
    for (id, _name, kind, protected) in records {
        if registry.plugins.iter().any(|record| record.id == id) {
            continue;
        }
        registry.plugins.push(PluginRecord {
            id: id.into(),
            version: "1.0.0".into(),
            source: PluginSource::BuiltIn,
            checksum: None,
            state: PluginState::Enabled,
            permissions: Vec::new(),
            last_error: None,
            protected,
        });
        let _ = kind;
    }
    if !registry
        .plugins
        .iter()
        .any(|record| record.id == DEFAULT_SKIN_ID)
    {
        registry.plugins.push(PluginRecord {
            id: DEFAULT_SKIN_ID.into(),
            version: "1.0.0".into(),
            source: PluginSource::BuiltIn,
            checksum: None,
            state: PluginState::Enabled,
            permissions: Vec::new(),
            last_error: None,
            protected: true,
        });
    }
    registry.plugins.retain(|record| {
        !matches!(record.state, PluginState::Removed) || record.id == DEFAULT_SKIN_ID
    });
}

fn builtin_skin_manifest(id: &str, display_name: &str) -> PluginManifest {
    PluginManifest {
        manifest_version: MANIFEST_VERSION,
        id: id.into(),
        version: "1.0.0".into(),
        display_name: display_name.into(),
        kind: "skin".into(),
        host_api: HOST_API.into(),
        permissions: Vec::new(),
        contributes: vec![PluginContribution {
            id: format!("{id}.skin"),
            contribution_type: ContributionType::Skins,
            resource: Some(format!("skins/{id}.png")),
            thumbnail: Some(format!("skins/{id}-thumb.png")),
            width: Some(pet_skins::BASE_LOGICAL_WIDTH),
            height: Some(pet_skins::BASE_LOGICAL_HEIGHT),
            label: Some(display_name.into()),
            actions: Vec::new(),
        }],
    }
}

fn builtin_manifest(id: &str) -> PluginManifest {
    if let Some(skin) = pet_skins::skin_by_id(id) {
        return builtin_skin_manifest(id, skin.display_name);
    }
    let (display_name, kind, contribution_types) = builtin_metadata(id);
    PluginManifest {
        manifest_version: MANIFEST_VERSION,
        id: id.into(),
        version: "1.0.0".into(),
        display_name,
        kind,
        host_api: HOST_API.into(),
        permissions: Vec::new(),
        contributes: contribution_types
            .into_iter()
            .enumerate()
            .map(|(index, contribution_type)| PluginContribution {
                id: format!("{id}.{}", contribution_type.label()),
                contribution_type,
                resource: None,
                thumbnail: None,
                width: None,
                height: None,
                label: Some(format!("官方贡献 {}", index + 1)),
                actions: Vec::new(),
            })
            .collect(),
    }
}

fn builtin_metadata(id: &str) -> (String, String, Vec<ContributionType>) {
    if let Some(skin) = pet_skins::skin_by_id(id) {
        return (
            skin.display_name.into(),
            "skin".into(),
            vec![ContributionType::Skins],
        );
    }
    let name = match id {
        "core-host" => "核心宿主",
        "plugin-manager" => "插件管理",
        "official-usage" => "使用统计",
        "official-quick-panel" => "快捷面板",
        "official-size-settings" => "大小设置",
        _ => id,
    };
    let contributions = match id {
        "core-host" => vec![ContributionType::Menus],
        "plugin-manager" => vec![ContributionType::PanelCards, ContributionType::Menus],
        "official-usage" => vec![ContributionType::PanelCards, ContributionType::Menus],
        "official-quick-panel" => vec![
            ContributionType::PanelCards,
            ContributionType::Settings,
            ContributionType::Menus,
        ],
        "official-size-settings" => vec![ContributionType::Settings],
        _ => Vec::new(),
    };
    (name.into(), "official".into(), contributions)
}

fn upsert_record(registry: &mut PluginRegistryFile, replacement: PluginRecord) -> usize {
    if let Some(index) = registry
        .plugins
        .iter()
        .position(|record| record.id == replacement.id)
    {
        registry.plugins[index] = replacement;
        index
    } else {
        registry.plugins.push(replacement);
        registry.plugins.len() - 1
    }
}

fn find_record_mut<'a>(
    registry: &'a mut PluginRegistryFile,
    id: &str,
) -> Result<&'a mut PluginRecord, PluginError> {
    validate_id(id, "plugin_id_invalid")?;
    registry
        .plugins
        .iter_mut()
        .find(|record| record.id == id && !matches!(record.state, PluginState::Removed))
        .ok_or_else(|| PluginError::new("plugin_not_found", "未找到该插件。"))
}

fn find_record_index(registry: &PluginRegistryFile, id: &str) -> Result<usize, PluginError> {
    validate_id(id, "plugin_id_invalid")?;
    registry
        .plugins
        .iter()
        .position(|record| record.id == id && !matches!(record.state, PluginState::Removed))
        .ok_or_else(|| PluginError::new("plugin_not_found", "未找到该插件。"))
}

fn summary_for_record(state: &PluginManagerState, record: PluginRecord) -> PluginSummary {
    let (display_name, kind, contributions) = state
        .packages
        .get(&record.id)
        .map(|package| {
            (
                package.manifest.display_name.clone(),
                package.manifest.kind.clone(),
                package.manifest.contribution_types(),
            )
        })
        .unwrap_or_else(|| builtin_metadata(&record.id));
    PluginSummary {
        id: record.id,
        display_name,
        version: record.version,
        kind,
        source: record.source,
        state: record.state,
        permissions: record.permissions,
        contributions,
        last_error: record.last_error,
        protected: record.protected,
        installed: true,
    }
}

fn enabled_skin_count(state: &PluginManagerState) -> usize {
    state
        .registry
        .plugins
        .iter()
        .filter(|record| {
            if !matches!(record.state, PluginState::Enabled) {
                return false;
            }
            pet_skins::skin_by_id(&record.id).is_some()
                || state.packages.get(&record.id).is_some_and(|package| {
                    package.manifest.contributes.iter().any(|contribution| {
                        matches!(contribution.contribution_type, ContributionType::Skins)
                    })
                })
        })
        .count()
}

fn write_package_files(root: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<(), PluginError> {
    for (name, bytes) in files {
        validate_package_path(name)?;
        let destination = root.join(name);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&destination)
            .map_err(PluginError::from)?;
        file.write_all(bytes).map_err(PluginError::from)?;
        file.sync_all().map_err(PluginError::from)?;
    }
    Ok(())
}

fn replace_directory(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        fs::rename(source, destination)
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, destination)
    }
}

fn validate_local_import_path(path: &Path) -> Result<(), PluginError> {
    if path.as_os_str().is_empty()
        || !path.is_file()
        || path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("petpack"))
    {
        return Err(PluginError::new(
            "plugin_import_path_invalid",
            "请选择本地 .petpack 文件。",
        ));
    }
    Ok(())
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        encoded.push(TABLE[(first >> 2) as usize] as char);
        encoded.push(TABLE[(((first & 0b11) << 4) | (second >> 4)) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            TABLE[(((second & 0b1111) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            TABLE[(third & 0b111111) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write};

    use image::{ImageBuffer, Rgba};
    use tempfile::tempdir;
    use zip::{ZipWriter, write::SimpleFileOptions};

    use super::*;

    fn valid_manifest() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "manifestVersion": 1,
            "id": "demo-skin",
            "version": "1.2.3",
            "displayName": "示例皮肤",
            "kind": "skin",
            "hostApi": "1",
            "permissions": [],
            "contributes": [{
                "id": "demo-skin.skin",
                "type": "skins",
                "resource": "skins/pet.png",
                "thumbnail": "skins/thumb.png",
                "width": 200,
                "height": 220
            }]
        }))
        .unwrap()
    }

    fn png(size: u32) -> Vec<u8> {
        let mut image = ImageBuffer::from_pixel(size, size, Rgba([0, 0, 0, 0]));
        image.put_pixel(size / 2, size / 2, Rgba([255, 120, 20, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn petpack(manifest: &[u8], resource: &[u8], thumbnail: &[u8]) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(&mut bytes);
        let options = SimpleFileOptions::default();
        writer.start_file(MANIFEST_FILE, options).unwrap();
        writer.write_all(manifest).unwrap();
        writer.start_file("skins/pet.png", options).unwrap();
        writer.write_all(resource).unwrap();
        writer.start_file("skins/thumb.png", options).unwrap();
        writer.write_all(thumbnail).unwrap();
        writer.finish().unwrap();
        bytes.into_inner()
    }

    #[test]
    fn manifest_validation_rejects_unknown_fields_ids_permissions_and_host_api() {
        let mut value: serde_json::Value = serde_json::from_slice(&valid_manifest()).unwrap();
        value["unknown"] = true.into();
        assert_eq!(
            PluginManifest::parse(&serde_json::to_vec(&value).unwrap())
                .unwrap_err()
                .code,
            "plugin_manifest_invalid"
        );

        value = serde_json::from_slice(&valid_manifest()).unwrap();
        value["id"] = "../escape".into();
        assert_eq!(
            PluginManifest::parse(&serde_json::to_vec(&value).unwrap())
                .unwrap_err()
                .code,
            "plugin_manifest_invalid"
        );

        value = serde_json::from_slice(&valid_manifest()).unwrap();
        value["hostApi"] = "2".into();
        assert_eq!(
            PluginManifest::parse(&serde_json::to_vec(&value).unwrap())
                .unwrap_err()
                .code,
            "plugin_host_api_unsupported"
        );

        value = serde_json::from_slice(&valid_manifest()).unwrap();
        value["permissions"] = serde_json::json!(["network"]);
        assert_eq!(
            PluginManifest::parse(&serde_json::to_vec(&value).unwrap())
                .unwrap_err()
                .code,
            "plugin_permission_unsupported"
        );

        value = serde_json::from_slice(&valid_manifest()).unwrap();
        value["contributes"] = serde_json::json!({
            "telemetry": [{
                "id": "demo-skin.skin",
                "type": "skins",
                "resource": "skins/pet.png",
                "thumbnail": "skins/thumb.png",
                "width": 200,
                "height": 220
            }]
        });
        assert_eq!(
            PluginManifest::parse(&serde_json::to_vec(&value).unwrap())
                .unwrap_err()
                .code,
            "plugin_manifest_invalid"
        );
    }

    #[test]
    fn package_validation_accepts_valid_png_and_rejects_paths_duplicates_scripts_and_bad_images() {
        let bytes = petpack(&valid_manifest(), &png(32), &png(16));
        let package = validate_petpack(&bytes).unwrap();
        assert_eq!(package.manifest.id, "demo-skin");
        assert_eq!(package.files.len(), 3);
        assert_eq!(package.checksum.len(), 64);

        let mut path_manifest: serde_json::Value =
            serde_json::from_slice(&valid_manifest()).unwrap();
        path_manifest["contributes"][0]["resource"] = "../pet.png".into();
        assert_eq!(
            validate_petpack(&petpack(
                &serde_json::to_vec(&path_manifest).unwrap(),
                &png(32),
                &png(16)
            ))
            .unwrap_err()
            .code,
            "plugin_path_invalid"
        );

        let mut script = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(&mut script);
        writer
            .start_file(MANIFEST_FILE, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&valid_manifest()).unwrap();
        writer
            .start_file("run.js", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"alert(1)").unwrap();
        writer.finish().unwrap();
        assert_eq!(
            validate_petpack(&script.into_inner()).unwrap_err().code,
            "plugin_script_forbidden"
        );

        let opaque = ImageBuffer::from_pixel(16, 16, Rgba([1, 2, 3, 255]));
        let mut opaque_bytes = Vec::new();
        image::DynamicImage::ImageRgba8(opaque)
            .write_to(&mut Cursor::new(&mut opaque_bytes), ImageFormat::Png)
            .unwrap();
        assert_eq!(
            validate_petpack(&petpack(&valid_manifest(), &opaque_bytes, &png(16)))
                .unwrap_err()
                .code,
            "plugin_image_transparency_invalid"
        );
    }

    #[test]
    fn registry_bootstrap_is_recoverable_and_atomic() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("plugin-registry.json");
        fs::write(&path, "not-json").unwrap();
        let registry = load_registry(&path);
        assert_eq!(registry.version, REGISTRY_VERSION);
        assert!(registry.plugins.is_empty());
        let mut registry = PluginRegistryFile::default();
        registry.plugins.push(PluginRecord {
            id: "demo-skin".into(),
            version: "1.0.0".into(),
            source: PluginSource::LocalImport,
            checksum: None,
            state: PluginState::Disabled,
            permissions: Vec::new(),
            last_error: None,
            protected: false,
        });
        save_registry(&path, &registry).unwrap();
        assert_eq!(load_registry(&path).plugins[0].state, PluginState::Disabled);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn contribution_handles_revoke_in_reverse_lifecycle_order() {
        let registry = ContributionRegistry::default();
        let manifest = PluginManifest::parse(&valid_manifest()).unwrap();
        let handle = registry
            .register(&manifest.id, &manifest.contributes)
            .unwrap();
        assert_eq!(registry.list().len(), 1);
        handle.revoke();
        assert!(registry.list().is_empty());
    }

    #[test]
    fn contribution_conflict_does_not_leave_a_partial_registration() {
        let registry = ContributionRegistry::default();
        let contribution = PluginContribution {
            id: "shared.card".into(),
            contribution_type: ContributionType::PanelCards,
            resource: None,
            thumbnail: None,
            width: None,
            height: None,
            label: Some("共享卡片".into()),
            actions: Vec::new(),
        };
        let _first = registry.register("first", &[contribution.clone()]).unwrap();
        let error = match registry.register("second", &[contribution]) {
            Ok(_) => panic!("duplicate contribution should be rejected"),
            Err(error) => error,
        };
        assert_eq!(error.code, "plugin_contribution_conflict");
        assert_eq!(registry.list_for_plugin("first").len(), 1);
        assert!(registry.list_for_plugin("second").is_empty());
    }

    #[test]
    fn manager_bootstrap_has_one_enabled_skin_and_isolates_settings() {
        let directory = tempdir().unwrap();
        let manager = PluginManager::bootstrap(directory.path().to_path_buf()).unwrap();
        let summaries = manager.summaries();
        assert_eq!(
            summaries
                .iter()
                .filter(|item| item.kind == "skin" && item.state == PluginState::Enabled)
                .count(),
            1
        );
        assert!(manager.is_skin_enabled(DEFAULT_SKIN_ID));
        manager
            .write_settings("demo-skin", &serde_json::json!({"enabled": true}))
            .unwrap();
        assert_eq!(manager.read_settings("demo-skin").unwrap()["enabled"], true);
        assert_eq!(
            manager.read_settings("other-skin").unwrap(),
            serde_json::json!({})
        );
        assert_eq!(manager.catalog().len(), 3);
        assert!(
            manager
                .catalog()
                .iter()
                .find(|entry| entry.id == "simple-cloud")
                .is_some_and(|entry| entry.installed)
        );
        assert!(
            manager
                .catalog()
                .iter()
                .find(|entry| entry.id == "orange-dragon")
                .is_some_and(|entry| !entry.installed)
        );
    }

    #[test]
    fn installing_and_uninstalling_preserves_default_skin() {
        let directory = tempdir().unwrap();
        let manager = PluginManager::bootstrap(directory.path().to_path_buf()).unwrap();
        manager
            .install_package(&petpack(&valid_manifest(), &png(32), &png(16)))
            .unwrap();
        assert!(
            manager
                .summaries()
                .iter()
                .any(|item| item.id == "demo-skin" && item.state == PluginState::Installed)
        );
        manager.enable("demo-skin").unwrap();
        manager.disable(DEFAULT_SKIN_ID).unwrap();
        assert!(!manager.is_skin_enabled(DEFAULT_SKIN_ID));
        manager.enable(DEFAULT_SKIN_ID).unwrap();
        manager.disable("demo-skin").unwrap();
        manager.uninstall("demo-skin").unwrap();
        assert!(manager.is_skin_enabled(DEFAULT_SKIN_ID));
        assert_eq!(
            manager.uninstall(DEFAULT_SKIN_ID).unwrap_err().code,
            "plugin_protected"
        );
    }

    #[test]
    fn importing_reserved_core_or_official_ids_is_rejected() {
        let directory = tempdir().unwrap();
        let manager = PluginManager::bootstrap(directory.path().to_path_buf()).unwrap();
        let mut manifest: serde_json::Value = serde_json::from_slice(&valid_manifest()).unwrap();
        manifest["id"] = "simple-cloud".into();
        manifest["contributes"][0]["id"] = "simple-cloud.skin".into();
        let error = manager
            .install_package(&petpack(
                &serde_json::to_vec(&manifest).unwrap(),
                &png(32),
                &png(16),
            ))
            .unwrap_err();
        assert_eq!(error.code, "plugin_id_reserved");
    }

    #[test]
    fn registry_write_failure_cleans_temporary_file_without_replacing_old_state() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("plugin-registry.json");
        let registry = PluginRegistryFile::default();
        fs::write(&path, br#"{"version":1,"plugins":[]}"#).unwrap();
        let error = save_registry_with(&path, &registry, |_source, _destination| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "simulated failure",
            ))
        })
        .unwrap_err();
        assert_eq!(error.code, "plugin_io_error");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"version":1,"plugins":[]}"#
        );
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn checked_example_petpack_is_valid_and_declarative() {
        let package = validate_petpack(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../examples/petpack/demo-skin.petpack"
        )))
        .unwrap();
        assert_eq!(package.manifest.id, "example-skin");
        assert_eq!(package.manifest.permissions, Vec::<String>::new());
        assert_eq!(
            package.manifest.contribution_types(),
            vec![ContributionType::Skins]
        );
    }
}
