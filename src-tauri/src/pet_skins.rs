use std::sync::OnceLock;

use image::{ImageFormat, RgbaImage};

pub const DEFAULT_SKIN_ID: &str = "simple-cloud";
pub const BASE_LOGICAL_WIDTH: u32 = 200;
pub const BASE_LOGICAL_HEIGHT: u32 = 220;

const MIN_MAIN_DIMENSION: u32 = 64;
const MAX_MAIN_DIMENSION: u32 = 1024;
const MIN_THUMBNAIL_DIMENSION: u32 = 32;
const MAX_THUMBNAIL_DIMENSION: u32 = 256;

const SIMPLE_CLOUD: &[u8] = include_bytes!("../assets/skins/simple-cloud.png");
const SIMPLE_CLOUD_THUMBNAIL: &[u8] = include_bytes!("../assets/skins/simple-cloud-thumb.png");
const ORANGE_DRAGON: &[u8] = include_bytes!("../assets/skins/orange-dragon.png");
const ORANGE_DRAGON_THUMBNAIL: &[u8] = include_bytes!("../assets/skins/orange-dragon-thumb.png");
const CALICO_CAT: &[u8] = include_bytes!("../assets/skins/calico-cat.png");
const CALICO_CAT_THUMBNAIL: &[u8] = include_bytes!("../assets/skins/calico-cat-thumb.png");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkinDefinition {
    pub id: &'static str,
    pub display_name: &'static str,
    pub logical_width: u32,
    pub logical_height: u32,
    pub png: &'static [u8],
    pub thumbnail_png: &'static [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkinAssetError(pub String);

impl std::fmt::Display for SkinAssetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SkinAssetError {}

const MANIFEST: [SkinDefinition; 3] = [
    SkinDefinition {
        id: "simple-cloud",
        display_name: "简洁云朵",
        logical_width: BASE_LOGICAL_WIDTH,
        logical_height: BASE_LOGICAL_HEIGHT,
        png: SIMPLE_CLOUD,
        thumbnail_png: SIMPLE_CLOUD_THUMBNAIL,
    },
    SkinDefinition {
        id: "orange-dragon",
        display_name: "橙色小龙",
        logical_width: BASE_LOGICAL_WIDTH,
        logical_height: BASE_LOGICAL_HEIGHT,
        png: ORANGE_DRAGON,
        thumbnail_png: ORANGE_DRAGON_THUMBNAIL,
    },
    SkinDefinition {
        id: "calico-cat",
        display_name: "三花猫",
        logical_width: BASE_LOGICAL_WIDTH,
        logical_height: BASE_LOGICAL_HEIGHT,
        png: CALICO_CAT,
        thumbnail_png: CALICO_CAT_THUMBNAIL,
    },
];

pub fn available_skins() -> &'static [SkinDefinition] {
    static VALIDATED: OnceLock<Vec<SkinDefinition>> = OnceLock::new();
    VALIDATED
        .get_or_init(|| validate_manifest(&MANIFEST))
        .as_slice()
}

pub fn skin_by_id(id: &str) -> Option<SkinDefinition> {
    available_skins().iter().copied().find(|skin| skin.id == id)
}

pub fn decode_skin(skin: SkinDefinition) -> Result<RgbaImage, SkinAssetError> {
    decode_png(skin.png, "主资源")
}

pub fn thumbnail_data_url(skin: SkinDefinition) -> String {
    format!(
        "data:image/png;base64,{}",
        encode_base64(skin.thumbnail_png)
    )
}

fn validate_manifest(manifest: &[SkinDefinition]) -> Vec<SkinDefinition> {
    let mut valid = Vec::with_capacity(manifest.len());
    for skin in manifest {
        if valid
            .iter()
            .any(|known: &SkinDefinition| known.id == skin.id)
        {
            continue;
        }
        if validate_skin(*skin).is_ok() {
            valid.push(*skin);
        }
    }
    valid
}

fn validate_skin(skin: SkinDefinition) -> Result<(), SkinAssetError> {
    if skin.id.is_empty()
        || !skin
            .id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
    {
        return Err(SkinAssetError("皮肤 ID 必须是稳定的小写连字符标识".into()));
    }
    if skin.display_name.is_empty()
        || skin.logical_width != BASE_LOGICAL_WIDTH
        || skin.logical_height != BASE_LOGICAL_HEIGHT
    {
        return Err(SkinAssetError("皮肤缺少有效的显示名称或基准尺寸".into()));
    }
    let main = decode_png(skin.png, "主资源")?;
    validate_main_image(&main)?;
    let thumbnail = decode_png(skin.thumbnail_png, "缩略图")?;
    validate_thumbnail(&thumbnail)?;
    Ok(())
}

fn decode_png(bytes: &[u8], label: &str) -> Result<RgbaImage, SkinAssetError> {
    image::load_from_memory_with_format(bytes, ImageFormat::Png)
        .map_err(|error| SkinAssetError(format!("{label}无法解码为 PNG：{error}")))
        .map(|image| image.into_rgba8())
}

fn validate_main_image(image: &RgbaImage) -> Result<(), SkinAssetError> {
    if !(MIN_MAIN_DIMENSION..=MAX_MAIN_DIMENSION).contains(&image.width())
        || !(MIN_MAIN_DIMENSION..=MAX_MAIN_DIMENSION).contains(&image.height())
    {
        return Err(SkinAssetError("主资源尺寸超出允许范围".into()));
    }
    validate_transparency(image)
}

fn validate_thumbnail(image: &RgbaImage) -> Result<(), SkinAssetError> {
    if !(MIN_THUMBNAIL_DIMENSION..=MAX_THUMBNAIL_DIMENSION).contains(&image.width())
        || !(MIN_THUMBNAIL_DIMENSION..=MAX_THUMBNAIL_DIMENSION).contains(&image.height())
    {
        return Err(SkinAssetError("缩略图尺寸超出允许范围".into()));
    }
    validate_transparency(image)
}

fn validate_transparency(image: &RgbaImage) -> Result<(), SkinAssetError> {
    let corners = [
        image.get_pixel(0, 0)[3],
        image.get_pixel(image.width() - 1, 0)[3],
        image.get_pixel(0, image.height() - 1)[3],
        image.get_pixel(image.width() - 1, image.height() - 1)[3],
    ];
    if corners.iter().any(|alpha| *alpha != 0) {
        return Err(SkinAssetError("资源四角必须透明".into()));
    }
    if !image.pixels().any(|pixel| pixel[3] > 0) {
        return Err(SkinAssetError("资源没有可见像素".into()));
    }
    if !image.pixels().any(|pixel| pixel[3] == 0) {
        return Err(SkinAssetError("资源必须保留透明边界".into()));
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
            TABLE[(third & 0b11_1111) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

#[cfg(test)]
mod tests {
    use image::{ColorType, ImageEncoder, Rgba};

    use super::*;

    fn png(width: u32, height: u32, fill: Rgba<u8>) -> Vec<u8> {
        let mut image = RgbaImage::from_pixel(width, height, fill);
        image.put_pixel(width / 2, height / 2, Rgba([255, 127, 0, 255]));
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(image.as_raw(), width, height, ColorType::Rgba8.into())
            .unwrap();
        bytes
    }

    fn fixture(id: &'static str, main: &'static [u8], thumbnail: &'static [u8]) -> SkinDefinition {
        SkinDefinition {
            id,
            display_name: "测试皮肤",
            logical_width: BASE_LOGICAL_WIDTH,
            logical_height: BASE_LOGICAL_HEIGHT,
            png: main,
            thumbnail_png: thumbnail,
        }
    }

    #[test]
    fn built_in_manifest_exposes_the_three_valid_skins_in_order() {
        let skins = available_skins();
        assert_eq!(
            skins.iter().map(|skin| skin.id).collect::<Vec<_>>(),
            ["simple-cloud", "orange-dragon", "calico-cat",]
        );
        assert!(skins.iter().all(|skin| validate_skin(*skin).is_ok()));
        assert!(thumbnail_data_url(skins[0]).starts_with("data:image/png;base64,"));
    }

    #[test]
    fn invalid_missing_and_noncompliant_resources_are_excluded() {
        let valid_main = Box::leak(png(64, 64, Rgba([0, 0, 0, 0])).into_boxed_slice());
        let valid_thumbnail = Box::leak(png(32, 32, Rgba([0, 0, 0, 0])).into_boxed_slice());
        let opaque_main = Box::leak(png(64, 64, Rgba([1, 2, 3, 255])).into_boxed_slice());
        let definitions = [
            fixture("valid", valid_main, valid_thumbnail),
            fixture("bad-png", b"not-a-png", valid_thumbnail),
            fixture("missing-thumbnail", valid_main, b""),
            fixture("opaque-corners", opaque_main, valid_thumbnail),
            fixture("valid", valid_main, valid_thumbnail),
        ];
        assert_eq!(
            validate_manifest(&definitions)
                .iter()
                .map(|skin| skin.id)
                .collect::<Vec<_>>(),
            ["valid"]
        );
    }
}
