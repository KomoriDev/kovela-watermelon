use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use color_quant::NeuQuant;
use image::{DynamicImage, ImageDecoder, ImageReader, Limits, imageops::FilterType};
use serde_json::{Value, json};
use std::io::Cursor;
use uuid::Uuid;

pub(crate) const PRODUCT: &str = "com.komoridev.watermelon";
pub(crate) const SIZES: [u16; 11] = [36, 46, 58, 72, 89, 108, 128, 149, 173, 192, 192];
pub(crate) const NAMES: [&str; 11] = [
    "葡萄",
    "樱桃",
    "橙子",
    "柠檬",
    "猕猴桃",
    "番茄",
    "桃子",
    "菠萝",
    "椰子",
    "西瓜",
    "大西瓜",
];
pub(crate) const MAX_BYTES: usize = 40 * 1024;
pub(crate) const CHUNK_BYTES: usize = 1536;
const MAX_INPUT: usize = 8 * 1024 * 1024;

pub(crate) fn indexed_preview(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() < 1037
        || bytes.len() > MAX_BYTES
        || bytes[..4] != [0x19, 0x0a, 0, 0]
        || bytes[10..12] != [0, 0]
    {
        return Err("图片预览加载失败".into());
    }
    let width = u16::from_le_bytes([bytes[4], bytes[5]]);
    if width == 0
        || width > 192
        || u16::from_le_bytes([bytes[6], bytes[7]]) != width
        || u16::from_le_bytes([bytes[8], bytes[9]]) != width
        || bytes.len() != 1036 + usize::from(width) * usize::from(width)
    {
        return Err("图片预览加载失败".into());
    }
    let mut rgba = image::RgbaImage::new(width.into(), width.into());
    for (index, pixel) in bytes[1036..].iter().zip(rgba.pixels_mut()) {
        let color = &bytes[12 + usize::from(*index) * 4..16 + usize::from(*index) * 4];
        *pixel = image::Rgba([color[2], color[1], color[0], color[3]]);
    }
    let mut output = Cursor::new(Vec::new());
    rgba.write_to(&mut output, image::ImageFormat::Png)
        .map_err(|_| "图片预览加载失败")?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(output.into_inner())
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewResult {
    Next,
    Complete,
    Restart,
}

pub(crate) struct PreviewRead {
    pub(crate) id: String,
    pub(crate) level: usize,
    pub(crate) bytes: Vec<u8>,
    pub(crate) retries: u8,
    pub(crate) restarts: u8,
    length: usize,
    width: u16,
}
impl PreviewRead {
    pub(crate) fn new(level: usize) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            level,
            bytes: Vec::new(),
            retries: 0,
            restarts: 0,
            length: 0,
            width: 0,
        }
    }
    /// 换 id 从零重读，避免重连前迟到的分块混入新图片；保留重读计数。
    pub(crate) fn restart(&mut self) {
        self.id = Uuid::new_v4().to_string();
        self.bytes.clear();
        self.retries = 0;
        self.restarts += 1;
        self.length = 0;
        self.width = 0;
    }
    pub(crate) fn packet(&self) -> String {
        json!({"v":1,"id":self.id,"type":"image-read","level":self.level,"offset":self.bytes.len()})
            .to_string()
    }
    pub(crate) fn accept(&mut self, value: &Value) -> Result<Option<PreviewResult>, String> {
        if value["v"] != 1
            || value["type"] != "image-preview"
            || value["id"] != self.id
            || value["level"] != self.level
        {
            return Ok(None);
        }
        if value["success"] != true {
            if matches!(value["error"].as_str(), Some("BUSY" | "NOT_READY")) {
                return Ok(None);
            }
            if value["error"] == "NO_PREVIEW" {
                return Ok(Some(PreviewResult::Restart));
            }
            return Err("图片预览加载失败".into());
        }
        if value["default"] == true {
            self.bytes.clear();
            return Ok(Some(PreviewResult::Complete));
        }
        let offset = value["offset"]
            .as_u64()
            .filter(|n| *n < MAX_BYTES as u64)
            .ok_or("图片预览加载失败")? as usize;
        if offset < self.bytes.len() {
            return Ok(None);
        }
        let length = value["length"]
            .as_u64()
            .filter(|n| *n <= MAX_BYTES as u64)
            .ok_or("图片预览加载失败")? as usize;
        let width = value["width"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= u64::from(SIZES[self.level]))
            .ok_or("图片预览加载失败")? as u16;
        if offset != self.bytes.len()
            || length != 1036 + usize::from(width) * usize::from(width)
            || (self.length != 0 && (length != self.length || width != self.width))
        {
            return Err("图片预览加载失败".into());
        }
        let data = value["data"]
            .as_str()
            .filter(|v| v.len() <= CHUNK_BYTES * 4 / 3)
            .ok_or("图片预览加载失败")?;
        let bytes = URL_SAFE_NO_PAD
            .decode(data)
            .map_err(|_| "图片预览加载失败")?;
        if bytes.len() != CHUNK_BYTES.min(length.saturating_sub(offset)) {
            return Err("图片预览加载失败".into());
        }
        if self.length == 0 {
            self.bytes.reserve_exact(length);
            self.length = length;
            self.width = width;
        }
        self.bytes.extend_from_slice(&bytes);
        self.retries = 0;
        Ok(Some(if self.bytes.len() == length {
            PreviewResult::Complete
        } else {
            PreviewResult::Next
        }))
    }
}
pub(crate) struct Converted {
    pub(crate) bytes: Vec<u8>,
    pub(crate) width: u16,
    pub(crate) preview: String,
}

pub(crate) fn convert(input: &[u8], level: usize) -> Result<Converted, String> {
    let size = *SIZES.get(level).ok_or("图片处理失败，请换一张试试")?;
    if input.is_empty() || input.len() > MAX_INPUT {
        return Err("请选择 8 MB 以内的图片".into());
    }
    let mut reader = ImageReader::new(Cursor::new(input))
        .with_guessed_format()
        .map_err(|_| "无法识别这张图片，请选择 PNG、JPG、WebP、BMP 或 GIF 格式")?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| "图片尺寸过大或格式不支持，请换一张试试")?;
    let orientation = decoder
        .orientation()
        .map_err(|_| "无法读取这张图片，请换一张试试")?;
    let mut original =
        DynamicImage::from_decoder(decoder).map_err(|_| "图片已损坏或尺寸过大，请换一张试试")?;
    original.apply_orientation(orientation);
    let mut rgba = original
        .resize_to_fill(size.into(), size.into(), FilterType::Lanczos3)
        .into_rgba8();
    drop(original);
    let radius = f32::from(size) / 2.0;
    for (x, y, pixel) in rgba.enumerate_pixels_mut() {
        let dx = x as f32 + 0.5 - radius;
        let dy = y as f32 + 0.5 - radius;
        let coverage = (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
        pixel[3] = (f32::from(pixel[3]) * coverage).round() as u8;
        if pixel[3] == 0 {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
        }
    }
    let quant = NeuQuant::new(3, 255, rgba.as_raw());
    let mut bytes = Vec::with_capacity(1036 + usize::from(size) * usize::from(size));
    // LVGL v9 I8: 12-byte header, 256 BGRA entries, byte indices, no compression flag.
    bytes.extend_from_slice(&[0x19, 0x0a, 0, 0]);
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    for i in 0..255 {
        let color = quant.lookup(i).unwrap();
        bytes.extend_from_slice(&[color[2], color[1], color[0], color[3]]);
    }
    for pixel in rgba.pixels() {
        bytes.push(if pixel[3] == 0 {
            0
        } else {
            (quant.index_of(&pixel.0) + 1) as u8
        });
    }
    if bytes.len() > MAX_BYTES {
        return Err("图片处理失败，请换一张试试".into());
    }
    let preview = indexed_preview(&bytes)?;
    Ok(Converted {
        bytes,
        width: size,
        preview,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Begin,
    Chunk,
    Commit,
    Reset,
}
impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::Begin => "begin",
            Self::Chunk => "chunk",
            Self::Commit => "commit",
            Self::Reset => "reset",
        }
    }
}

pub(crate) struct Transfer {
    pub(crate) id: String,
    pub(crate) addr: String,
    pub(crate) level: usize,
    bytes: Vec<u8>,
    width: u16,
    crc32: String,
    pub(crate) offset: usize,
    pub(crate) stage: Stage,
    pub(crate) retries: u8,
    pub(crate) sequence: u32,
    pub(crate) waiting: bool,
    pub(crate) failed: bool,
}
impl Transfer {
    pub(crate) fn new(addr: String, level: usize, image: Option<Converted>) -> Self {
        let (bytes, width, stage) = match image {
            Some(image) => (image.bytes, image.width, Stage::Begin),
            None => (Vec::new(), 0, Stage::Reset),
        };
        let crc32 = format!("{:08x}", crc32fast::hash(&bytes));
        Self {
            id: Uuid::new_v4().to_string(),
            addr,
            level,
            bytes,
            width,
            crc32,
            offset: 0,
            stage,
            retries: 0,
            sequence: 0,
            waiting: true,
            failed: false,
        }
    }
    pub(crate) fn packet(&self) -> String {
        let mut value = json!({"v": 1, "id": self.id, "type": format!("image-{}", self.stage.name()), "level": self.level});
        match self.stage {
            Stage::Begin => {
                value["width"] = self.width.into();
                value["height"] = self.width.into();
                value["length"] = self.bytes.len().into();
                value["crc32"] = self.crc32.clone().into();
            }
            Stage::Chunk => {
                let end = (self.offset + CHUNK_BYTES).min(self.bytes.len());
                value["offset"] = self.offset.into();
                value["data"] = URL_SAFE_NO_PAD.encode(&self.bytes[self.offset..end]).into();
            }
            Stage::Commit | Stage::Reset => {}
        }
        value.to_string()
    }
    pub(crate) fn progress(&self) -> u32 {
        if self.bytes.is_empty() {
            0
        } else {
            (self.offset * 100 / self.bytes.len()) as u32
        }
    }
    pub(crate) fn response(&mut self, value: &Value) -> Result<Option<bool>, String> {
        if value["v"] != 1
            || value["id"] != self.id
            || value["type"] != "image-result"
            || value["stage"] != self.stage.name()
            || !self.waiting
        {
            return Ok(None);
        }
        if value["success"] != true {
            if matches!(value["error"].as_str(), Some("NOT_READY" | "BUSY")) {
                return Ok(None);
            }
            self.waiting = false;
            self.failed = true;
            return Err(match value["error"].as_str() {
                Some("WRITE_FAILED") => "手环保存图片失败，原图片保持不变，请重试",
                Some("CHECKSUM_MISMATCH") => "图片传输出错，请重试",
                _ => "手环无法接收这张图片，请将合成大西瓜更新到最新版本后重试",
            }
            .into());
        }
        match self.stage {
            Stage::Begin | Stage::Chunk => {
                let offset = value["offset"].as_u64().ok_or("图片传输出错，请重试")? as usize;
                let expected = if self.stage == Stage::Begin {
                    self.bytes.len()
                } else {
                    (self.offset + CHUNK_BYTES).min(self.bytes.len())
                };
                if offset < self.offset || (self.stage == Stage::Chunk && offset == self.offset) {
                    return Ok(None);
                }
                if offset > expected || (self.stage == Stage::Chunk && offset != expected) {
                    return Err("图片传输出错，请重试".into());
                }
                self.offset = offset;
                self.stage = if offset == self.bytes.len() {
                    Stage::Commit
                } else {
                    Stage::Chunk
                };
                self.retries = 0;
                self.sequence += 1;
                Ok(Some(false))
            }
            Stage::Commit | Stage::Reset => {
                self.waiting = false;
                self.bytes.clear();
                Ok(Some(true))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgba, RgbaImage};
    fn encoded(width: u32, height: u32) -> Vec<u8> {
        let image = RgbaImage::from_pixel(width, height, Rgba([250, 20, 10, 255]));
        let mut output = Cursor::new(Vec::new());
        image.write_to(&mut output, ImageFormat::Png).unwrap();
        output.into_inner()
    }
    #[test]
    fn large_non_square_image_is_cropped_bounded_and_circular() {
        let result = convert(&encoded(900, 400), 10).unwrap();
        assert_eq!(result.width, 192);
        assert_eq!(result.bytes.len(), 1036 + 192 * 192);
        assert_eq!(result.bytes[1036], 0);
        let center = result.bytes[1036 + 96 * 192 + 96] as usize;
        assert!(center > 0);
        let pixel = &result.bytes[12 + center * 4..16 + center * 4];
        assert!(pixel[2] > 220 && pixel[0] < 30 && pixel[3] > 240);
    }
    #[test]
    fn preview_reader_preserves_exact_pixels_and_rejects_wrong_chunks() {
        let image = convert(&encoded(80, 100), 0).unwrap();
        let mut reader = PreviewRead::new(0);
        let mut reply = json!({"v":1,"id":reader.id,"type":"image-preview","level":0,"success":true,"width":36,"length":image.bytes.len(),"offset":0,
            "data":URL_SAFE_NO_PAD.encode(&image.bytes[..CHUNK_BYTES])});
        assert_eq!(reader.accept(&reply).unwrap(), Some(PreviewResult::Next));
        assert_eq!(reader.accept(&reply).unwrap(), None);
        reply["offset"] = CHUNK_BYTES.into();
        reply["data"] = URL_SAFE_NO_PAD.encode(&image.bytes[CHUNK_BYTES..]).into();
        assert_eq!(
            reader.accept(&reply).unwrap(),
            Some(PreviewResult::Complete)
        );
        assert_eq!(indexed_preview(&reader.bytes).unwrap(), image.preview);
        let mut invalid = PreviewRead::new(0);
        reply["id"] = invalid.id.clone().into();
        assert!(invalid.accept(&reply).is_err());
        assert!(invalid.bytes.is_empty());
        let data = base64::engine::general_purpose::STANDARD
            .decode(image.preview.split(',').nth(1).unwrap())
            .unwrap();
        let decoded = image::load_from_memory(&data).unwrap().into_rgba8();
        assert_eq!(decoded.get_pixel(0, 0)[3], 0);
        assert!(decoded.get_pixel(18, 18)[0] > 220);
    }

    #[test]
    fn reconnect_cache_loss_restarts_without_mixing_old_chunks() {
        let image = convert(&encoded(60, 50), 0).unwrap();
        let mut reader = PreviewRead::new(0);
        let first = json!({"v":1,"id":reader.id,"type":"image-preview","level":0,"success":true,
            "width":36,"length":image.bytes.len(),"offset":0,"data":URL_SAFE_NO_PAD.encode(&image.bytes[..CHUNK_BYTES])});
        assert_eq!(reader.accept(&first).unwrap(), Some(PreviewResult::Next));
        let lost = json!({"v":1,"id":reader.id,"type":"image-preview","level":0,"success":false,"error":"NO_PREVIEW"});
        assert_eq!(reader.accept(&lost).unwrap(), Some(PreviewResult::Restart));
        reader.restart();
        assert_eq!(reader.accept(&first).unwrap(), None);
        for offset in (0..image.bytes.len()).step_by(CHUNK_BYTES) {
            let end = (offset + CHUNK_BYTES).min(image.bytes.len());
            let reply = json!({"v":1,"id":reader.id,"type":"image-preview","level":0,"success":true,
                "width":36,"length":image.bytes.len(),"offset":offset,"data":URL_SAFE_NO_PAD.encode(&image.bytes[offset..end])});
            let expected = if end == image.bytes.len() {
                PreviewResult::Complete
            } else {
                PreviewResult::Next
            };
            assert_eq!(reader.accept(&reply).unwrap(), Some(expected));
        }
        assert_eq!(indexed_preview(&reader.bytes).unwrap(), image.preview);
        reader.restart();
        let mut first = first;
        first["id"] = reader.id.clone().into();
        assert_eq!(reader.accept(&first).unwrap(), Some(PreviewResult::Next));
        let reset = json!({"v":1,"id":reader.id,"type":"image-preview","level":0,"success":true,"default":true});
        assert_eq!(
            reader.accept(&reset).unwrap(),
            Some(PreviewResult::Complete)
        );
        assert!(reader.bytes.is_empty(), "恢复默认的回执不能保留半张旧图片");
    }

    #[test]
    fn transfer_only_advances_after_matching_next_offset() {
        let mut t = Transfer::new(
            "addr".into(),
            0,
            Some(convert(&encoded(10, 10), 0).unwrap()),
        );
        let begin = json!({"v":1,"id":t.id,"type":"image-result","stage":"begin","success":true,"offset":0});
        assert_eq!(t.response(&begin).unwrap(), Some(false));
        assert_eq!(t.stage, Stage::Chunk);
        let wrong = json!({"v":1,"id":t.id,"type":"image-result","stage":"chunk","success":true,"offset":5});
        assert!(t.response(&wrong).is_err());
        assert_eq!(t.offset, 0);
        let chunk = json!({"v":1,"id":t.id,"type":"image-result","stage":"chunk","success":true,"offset":1536});
        assert_eq!(t.response(&chunk).unwrap(), Some(false));
        assert_eq!(t.offset, 1536);
        assert_eq!(t.response(&chunk).unwrap(), None);
    }
}
