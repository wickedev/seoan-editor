use crate::{
    CoreError, ErrorCode, GeneratedCharacterPlan, HltBodyPlan, HmlDocument,
    generate_hlt_character_spans,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

const TIFF_PIXEL_OFFSET_RGB: u32 = 370;
const TIFF_PIXEL_OFFSET_GRAY: u32 = 364;

/// TIFF 픽셀 모드. 통특기는 원본 이미지의 색 모드를 보존한다 — pct_a 저장본은 RGB,
/// PCT 저장본은 8비트 그레이스케일이고 원본 DOCX 의 JPEG 도 전부 그레이스케일이었다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TiffPixels {
    Gray8,
    Rgb8,
    /// 1비트 흑백(WhiteIsZero, 행마다 바이트 맞춤). 국내출원 도면(300dpi 1비트 G4 TIFF)을 그대로 지킨다 —
    /// 제출 세트에서 G4 TIFF 로 다시 굽힌다(`export_fin::tiff_to_g4`). 8비트로 풀면 JPEG 로 나가 버린다.
    Bilevel,
}

impl TiffPixels {
    fn samples(self) -> u32 {
        match self {
            TiffPixels::Gray8 | TiffPixels::Bilevel => 1,
            TiffPixels::Rgb8 => 3,
        }
    }

    fn photometric(self) -> u32 {
        match self {
            TiffPixels::Bilevel => 0,
            TiffPixels::Gray8 => 1,
            TiffPixels::Rgb8 => 2,
        }
    }

    fn pixel_offset(self) -> u32 {
        match self {
            TiffPixels::Gray8 | TiffPixels::Bilevel => TIFF_PIXEL_OFFSET_GRAY,
            TiffPixels::Rgb8 => TIFF_PIXEL_OFFSET_RGB,
        }
    }

    /// 한 줄 바이트 수.
    fn row_bytes(self, width: u32) -> u64 {
        match self {
            TiffPixels::Bilevel => u64::from(width).div_ceil(8),
            _ => u64::from(width) * u64::from(self.samples()),
        }
    }
}
const IMAGE_RECORD_PREFIX: usize = 384;
const MAX_IMAGE_PAYLOAD_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TOTAL_IMAGE_PAYLOAD_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedImagePayload {
    pub image_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedImageObject {
    pub image_id: String,
    pub code: u32,
    pub payload_sha256: String,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub record_bytes: u32,
    pub record_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedImagePlan {
    pub spans: Vec<u8>,
    pub object_table: Vec<u8>,
    pub object_count: u32,
    pub images: Vec<GeneratedImageObject>,
}

pub fn load_hml_image_payloads_from_directory(
    document: &HmlDocument,
    root: &Path,
) -> Result<Vec<GeneratedImagePayload>, CoreError> {
    let root = root.canonicalize().map_err(|_| image_error("image_root"))?;
    let mut cache: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    let mut total = 0_u64;
    let mut payloads = Vec::with_capacity(document.images.len());
    for image in &document.images {
        let relative = Path::new(&image.name);
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return Err(image_error("image_relative_path"));
        }
        let path = root
            .join(relative)
            .canonicalize()
            .map_err(|_| image_error("image_source"))?;
        if !path.starts_with(&root) {
            return Err(image_error("image_path_escape"));
        }
        if !cache.contains_key(&path) {
            let metadata = fs::metadata(&path).map_err(|_| image_error("image_source"))?;
            if !metadata.is_file() || metadata.len() > MAX_IMAGE_PAYLOAD_BYTES {
                return Err(image_error("image_payload_limit"));
            }
            total = total
                .checked_add(metadata.len())
                .filter(|value| *value <= MAX_TOTAL_IMAGE_PAYLOAD_BYTES)
                .ok_or_else(|| image_error("image_total_limit"))?;
            let bytes = fs::read(&path).map_err(|_| image_error("image_source"))?;
            if bytes.len() as u64 != metadata.len() {
                return Err(image_error("image_payload_size"));
            }
            cache.insert(path.clone(), bytes);
        }
        payloads.push(GeneratedImagePayload {
            image_id: image.id.clone(),
            bytes: cache
                .get(&path)
                .ok_or_else(|| image_error("image_cache"))?
                .clone(),
        });
    }
    Ok(payloads)
}

pub fn plan_hlt_images(
    document: &HmlDocument,
    body: &HltBodyPlan,
    characters: &GeneratedCharacterPlan,
    payloads: &[GeneratedImagePayload],
) -> Result<GeneratedImagePlan, CoreError> {
    let mut payload_map = BTreeMap::new();
    for payload in payloads {
        if payload_map
            .insert(payload.image_id.as_str(), payload.bytes.as_slice())
            .is_some()
        {
            return Err(image_error("duplicate_payload"));
        }
    }
    let mut images: Vec<_> = document.images.iter().collect();
    images.sort_by_key(|image| image.occurrence);
    let mut ids = BTreeSet::new();
    for (expected, image) in images.iter().enumerate() {
        if image.occurrence != expected as u32 || !ids.insert(image.id.as_str()) {
            return Err(image_error("image_occurrence_order"));
        }
    }
    let anchor_ids: BTreeSet<_> = body
        .images
        .iter()
        .map(|anchor| anchor.image_id.as_str())
        .collect();
    if anchor_ids != ids {
        return Err(image_error("image_anchor_set"));
    }

    let mut object_table = characters.object_table.clone();
    let mut image_codes = BTreeMap::new();
    let mut objects = Vec::with_capacity(images.len());
    for (index, image) in images.into_iter().enumerate() {
        let payload = payload_map
            .get(image.id.as_str())
            .ok_or_else(|| image_error("missing_payload"))?;
        let bilevel = source_bilevel_dpi(payload);
        let decoded =
            flatten_on_white(image::load_from_memory(payload).map_err(|_| image_error("decode"))?);
        // 통특기는 원본의 색 모드를 보존한다 (PCT 저장본 그레이스케일, 옛 저장본 RGB).
        let gray = matches!(
            decoded.color(),
            image::ColorType::L8
                | image::ColorType::L16
                | image::ColorType::La8
                | image::ColorType::La16
        );
        let (mode, raster) = if bilevel.is_some() {
            (TiffPixels::Bilevel, pack_bilevel(&decoded.to_luma8()))
        } else if gray {
            (TiffPixels::Gray8, decoded.to_luma8().into_raw())
        } else {
            (TiffPixels::Rgb8, decoded.to_rgb8().into_raw())
        };
        let (pixel_width, pixel_height) = (decoded.width(), decoded.height());
        let code = characters
            .object_count
            .checked_add(index as u32)
            .ok_or_else(|| image_error("image_code_overflow"))?;
        let ext = if bilevel.is_some() { "tif" } else { "jpg" };
        let path = format!(r"C:\KIPONET\NKEDITOR\TEMP\SEOAN\pat{code:05}.{ext}");
        let secondary = match (image.secondary_width, image.secondary_height) {
            (Some(width), Some(height)) => Some((width, height)),
            _ => None,
        };
        let record = build_hlt_image_record_dpi(
            bilevel.unwrap_or(96),
            &path,
            pixel_width,
            pixel_height,
            &raster,
            mode,
            image.width.unwrap_or(pixel_width),
            image.height.unwrap_or(pixel_height),
            secondary,
        )?;
        let record_bytes = u32::try_from(record.len()).map_err(|_| image_error("record_size"))?;
        objects.push(GeneratedImageObject {
            image_id: image.id.clone(),
            code,
            payload_sha256: format!("{:X}", Sha256::digest(payload)),
            pixel_width,
            pixel_height,
            record_bytes,
            record_sha256: format!("{:X}", Sha256::digest(&record)),
        });
        object_table.extend_from_slice(&record);
        image_codes.insert(image.id.clone(), code);
    }
    if payload_map.len() != ids.len() {
        return Err(image_error("unused_payload"));
    }
    let object_count = characters
        .object_count
        .checked_add(objects.len() as u32)
        .ok_or_else(|| image_error("object_count_overflow"))?;
    object_table[1..5].copy_from_slice(&object_count.to_le_bytes());
    let spans = generate_hlt_character_spans(body, characters, &image_codes)?;
    Ok(GeneratedImagePlan {
        spans,
        object_table,
        object_count,
        images: objects,
    })
}

/// 원본이 1비트 TIFF 면 그 해상도(dpi, 없으면 300)를 돌려준다. 국내출원 도면 규격(별지 제17호: 300~400dpi
/// 흑백 TIFF)이라 그대로 흑백으로 지킨다.
fn source_bilevel_dpi(bytes: &[u8]) -> Option<u32> {
    let little = match bytes.get(..4)? {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };
    let u16_at = |at: usize| {
        let b = bytes.get(at..at + 2)?;
        Some(if little {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |at: usize| {
        let b = bytes.get(at..at + 4)?;
        Some(if little {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        })
    };
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    let (mut bits, mut samples, mut xres) = (1_u32, 1_u32, None);
    for k in 0..count {
        let at = ifd + 2 + 12 * k;
        let (tag, kind) = (u16_at(at)?, u16_at(at + 2)?);
        let value = if kind == 3 {
            u32::from(u16_at(at + 8)?)
        } else {
            u32_at(at + 8)?
        };
        match tag {
            258 => bits = value,
            277 => samples = value,
            282 if kind == 5 => {
                let off = value as usize;
                let (num, den) = (u32_at(off)?, u32_at(off + 4)?.max(1));
                xres = Some((num + den / 2) / den);
            }
            _ => {}
        }
    }
    (bits == 1 && samples == 1).then(|| xres.filter(|d| *d > 0).unwrap_or(300))
}

/// 8비트 회색 → 1비트(WhiteIsZero: 비트 1 = 검정), 행마다 바이트 맞춤.
fn pack_bilevel(gray: &image::GrayImage) -> Vec<u8> {
    let (width, height) = gray.dimensions();
    let row = width.div_ceil(8) as usize;
    let mut out = vec![0_u8; row * height as usize];
    for (x, y, pixel) in gray.enumerate_pixels() {
        if pixel.0[0] < 128 {
            out[y as usize * row + x as usize / 8] |= 0x80 >> (x % 8);
        }
    }
    out
}

/// 투명도가 있는 그림을 흰 바탕에 합성한다(없으면 그대로).
///
/// `to_rgb8` · `to_luma8` 는 알파를 버리고 색값만 남긴다. 투명 픽셀의 색값은 흔히 검정(0,0,0)이라
/// 워드에서는 흰 바탕으로 보이던 그림 가장자리가 검은 선이 된다. 회색조(La)는 회색조(L8)로 남긴다.
pub(crate) fn flatten_on_white(image: image::DynamicImage) -> image::DynamicImage {
    if !image.color().has_alpha() {
        return image;
    }
    let gray = matches!(
        image.color(),
        image::ColorType::La8 | image::ColorType::La16
    );
    let rgba = image.to_rgba8();
    let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
    for (src, dst) in rgba.pixels().zip(rgb.pixels_mut()) {
        let alpha = u32::from(src.0[3]);
        for channel in 0..3 {
            let value = u32::from(src.0[channel]) * alpha + 255 * (255 - alpha);
            dst.0[channel] = ((value + 127) / 255) as u8;
        }
    }
    let flat = image::DynamicImage::ImageRgb8(rgb);
    if gray {
        image::DynamicImage::ImageLuma8(flat.to_luma8())
    } else {
        flat
    }
}

pub fn build_rgb_tiff(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, CoreError> {
    build_tiff(width, height, pixels, TiffPixels::Rgb8)
}

pub fn build_tiff(
    width: u32,
    height: u32,
    pixels: &[u8],
    mode: TiffPixels,
) -> Result<Vec<u8>, CoreError> {
    build_tiff_dpi(width, height, pixels, mode, 96)
}

/// `build_tiff` 에 해상도(dpi)를 정해 준다. 1비트 도면은 원본 300dpi 를 지켜야 쪽 배치 · 제출 크기가 맞다.
pub fn build_tiff_dpi(
    width: u32,
    height: u32,
    pixels: &[u8],
    mode: TiffPixels,
    dpi: u32,
) -> Result<Vec<u8>, CoreError> {
    if width == 0 || height == 0 {
        return Err(image_error("zero_dimension"));
    }
    let pixel_offset = mode.pixel_offset();
    let expected = mode
        .row_bytes(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| image_error("pixel_size_overflow"))?;
    if expected != pixels.len() as u64 || expected > u64::from(u32::MAX) {
        return Err(image_error("pixel_length"));
    }
    // 그레이스케일은 BitsPerSample 을 인라인 SHORT 로 적어 254바이트 배열이 없다.
    // 그래서 유리수/문자열 값이 6바이트 앞당겨지고 픽셀이 364 에서 시작한다(네이티브와 동일).
    let gray = matches!(mode, TiffPixels::Gray8 | TiffPixels::Bilevel);
    let bits = match mode {
        TiffPixels::Bilevel => (3_u16, 1_u32, 1_u32),
        TiffPixels::Gray8 => (3, 1, 8),
        TiffPixels::Rgb8 => (3, 3, 254),
    };
    let (xres, yres, software, datetime, artist) = if gray {
        (254_u32, 262_u32, 270_u32, 300_u32, 320_u32)
    } else {
        (260, 268, 276, 306, 326)
    };
    let entries = [
        (0x00fe_u16, 4_u16, 1_u32, 2_u32),
        (0x0100, 4, 1, width),
        (0x0101, 4, 1, height),
        (0x0102, bits.0, bits.1, bits.2),
        (0x0103, 3, 1, 1),
        (0x0106, 3, 1, mode.photometric()),
        (0x010a, 3, 1, 1),
        (0x0111, 4, 1, pixel_offset),
        (0x0112, 3, 1, 1),
        (0x0115, 3, 1, mode.samples()),
        (0x0116, 3, 1, height),
        (0x0117, 4, 1, expected as u32),
        (0x011a, 5, 1, xres),
        (0x011b, 5, 1, yres),
        (0x011c, 3, 1, 1),
        (0x0128, 3, 1, 2),
        (0x0129, 3, 2, 0x0001_0000),
        (0x0131, 2, 30, software),
        (0x0132, 2, 20, datetime),
        (0x013b, 2, 44, artist),
    ];
    let mut out = Vec::with_capacity(pixel_offset as usize + pixels.len());
    out.extend_from_slice(b"II*\0");
    push_u32(&mut out, 8);
    push_u16(&mut out, entries.len() as u16);
    for (tag, field_type, count, value) in entries {
        push_u16(&mut out, tag);
        push_u16(&mut out, field_type);
        push_u32(&mut out, count);
        push_u32(&mut out, value);
    }
    push_u32(&mut out, 0);
    if !gray {
        for value in [8_u16, 8, 8] {
            push_u16(&mut out, value);
        }
    }
    for value in [dpi, 1, dpi, 1] {
        push_u32(&mut out, value);
    }
    fixed_ascii(&mut out, b"seoan HLT\0", 30)?;
    fixed_ascii(&mut out, b"2026:08:21 00:00:00\0", 20)?;
    fixed_ascii(&mut out, b"seoan\0", 44)?;
    if out.len() != pixel_offset as usize {
        return Err(image_error("tiff_header_size"));
    }
    out.extend_from_slice(pixels);
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub fn build_hlt_image_record(
    path: &str,
    width: u32,
    height: u32,
    pixels: &[u8],
    mode: TiffPixels,
    display_width: u32,
    display_height: u32,
    secondary: Option<(u32, u32)>,
) -> Result<Vec<u8>, CoreError> {
    build_hlt_image_record_dpi(
        96,
        path,
        width,
        height,
        pixels,
        mode,
        display_width,
        display_height,
        secondary,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_hlt_image_record_dpi(
    dpi: u32,
    path: &str,
    width: u32,
    height: u32,
    pixels: &[u8],
    mode: TiffPixels,
    display_width: u32,
    display_height: u32,
    secondary: Option<(u32, u32)>,
) -> Result<Vec<u8>, CoreError> {
    if !path.is_ascii() || path.len() + 1 > 260 {
        return Err(image_error("image_path"));
    }
    let tiff = build_tiff_dpi(width, height, pixels, mode, dpi)?;
    let tiff_len = u32::try_from(tiff.len()).map_err(|_| image_error("tiff_size"))?;
    let mut prefix = vec![0_u8; IMAGE_RECORD_PREFIX];
    put_u32(&mut prefix, 0, 128);
    put_u32(&mut prefix, 40, 3);
    put_u32(&mut prefix, 48, display_width.max(1));
    put_u32(&mut prefix, 52, display_height.max(1));
    prefix[64..68].copy_from_slice(&[0xff, 0xff, 0xff, 0]);
    put_u32(&mut prefix, 80, 1);
    put_u32(&mut prefix, 100, 1);
    prefix[104..104 + path.len()].copy_from_slice(path.as_bytes());
    let (secondary_width, secondary_height) = secondary.unwrap_or((
        width.checked_add(2).unwrap_or(width) / 5,
        height.checked_add(2).unwrap_or(height) / 5,
    ));
    put_u32(&mut prefix, 364, secondary_height.max(1));
    put_u32(&mut prefix, 368, secondary_width.max(1));
    put_u32(&mut prefix, 380, tiff_len);
    prefix.extend_from_slice(&tiff);
    Ok(prefix)
}

fn fixed_ascii(out: &mut Vec<u8>, value: &[u8], size: usize) -> Result<(), CoreError> {
    if !value.is_ascii() || value.len() > size {
        return Err(image_error("ascii_metadata"));
    }
    out.extend_from_slice(value);
    out.resize(out.len() + size - value.len(), 0);
    Ok(())
}

fn put_u32(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn image_error(reason: &str) -> CoreError {
    let mut error = CoreError::new(ErrorCode::StructureInvalid, "generate_images", false);
    error.details.insert("reason".into(), reason.into());
    error
}

#[cfg(test)]
mod tests {
    use super::flatten_on_white;

    #[test]
    fn transparent_edge_becomes_white_not_black() {
        // 맨 아래 줄만 완전 투명(색값 검정) — 흔한 SVG 래스터 결과
        let mut rgba = image::RgbaImage::from_pixel(4, 3, image::Rgba([255, 255, 255, 255]));
        rgba.put_pixel(1, 1, image::Rgba([0, 0, 0, 255]));
        for x in 0..4 {
            rgba.put_pixel(x, 2, image::Rgba([0, 0, 0, 0]));
        }
        let flat = flatten_on_white(image::DynamicImage::ImageRgba8(rgba));
        assert_eq!(flat.color(), image::ColorType::Rgb8);
        let rgb = flat.to_rgb8();
        assert!((0..4).all(|x| rgb.get_pixel(x, 2).0 == [255, 255, 255]));
        assert_eq!(rgb.get_pixel(1, 1).0, [0, 0, 0], "불투명한 선은 그대로");
    }

    #[test]
    fn gray_with_alpha_stays_gray_and_opaque_is_untouched() {
        let la = image::GrayAlphaImage::from_pixel(2, 1, image::LumaA([0, 0]));
        let flat = flatten_on_white(image::DynamicImage::ImageLumaA8(la));
        assert_eq!(flat.color(), image::ColorType::L8);
        assert_eq!(flat.to_luma8().get_pixel(0, 0).0, [255]);
        let rgb = image::DynamicImage::ImageRgb8(image::RgbImage::new(1, 1));
        assert_eq!(flatten_on_white(rgb.clone()), rgb);
    }
}
