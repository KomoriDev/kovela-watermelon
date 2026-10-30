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
const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// 手环只收 PNG：部分设备（小米手环 9/9 Pro 等）不支持 LVGL v9 `.bin`，统一用调色板 PNG。
fn data_url(bytes: &[u8]) -> String {
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// 256 色调色板（含透明度）+ 逐像素索引，编码成 8 位调色板 PNG。
fn encode_indexed(width: u16, palette: &[[u8; 4]; 256], pixels: &[u8]) -> Result<Vec<u8>, String> {
    const FAILED: &str = "图片处理失败，请换一张试试";
    let mut rgb = Vec::with_capacity(768);
    let mut alpha = Vec::with_capacity(256);
    for color in palette {
        rgb.extend_from_slice(&color[..3]);
        alpha.push(color[3]);
    }
    while alpha.last() == Some(&255) {
        alpha.pop();
    }
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width.into(), width.into());
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_palette(rgb);
        if !alpha.is_empty() {
            encoder.set_trns(alpha);
        }
        encoder.set_compression(png::Compression::High);
        let mut writer = encoder.write_header().map_err(|_| FAILED)?;
        writer.write_image_data(pixels).map_err(|_| FAILED)?;
        writer.finish().map_err(|_| FAILED)?;
    }
    if output.len() > MAX_BYTES {
        return Err(FAILED.into());
    }
    Ok(output)
}

/// 校验 PNG 签名、IHDR 为 `width` 见方并能完整解码，返回可显示的预览。
fn png_preview(bytes: &[u8], width: u16) -> Result<String, String> {
    if bytes.len() < 33
        || bytes.len() > MAX_BYTES
        || bytes[..8] != PNG_SIGNATURE
        || &bytes[12..16] != b"IHDR"
        || bytes[16..20] != u32::from(width).to_be_bytes()
        || bytes[20..24] != u32::from(width).to_be_bytes()
        || image::load_from_memory_with_format(bytes, image::ImageFormat::Png).is_err()
    {
        return Err("图片预览加载失败".into());
    }
    Ok(data_url(bytes))
}

/// 内置皮肤：从 IHDR 取边长后按同样的规则校验。
pub(crate) fn bundled_preview(bytes: &[u8]) -> Result<String, String> {
    let width = bytes
        .get(16..20)
        .and_then(|w| u16::try_from(u32::from_be_bytes(w.try_into().ok()?)).ok())
        .ok_or("图片预览加载失败")?;
    png_preview(bytes, width)
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
    /// 回读完成后的预览：校验后直接显示 PNG。
    pub(crate) fn preview(&self) -> Result<String, String> {
        png_preview(&self.bytes, self.width)
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
        // 手环上残留的旧 `.bin` 图片（无 format 字段）一律不再预览。
        if value["format"] != "png" {
            return Err("图片预览加载失败".into());
        }
        if offset != self.bytes.len()
            || length < 33
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
impl Converted {
    /// 内置皮肤：已是调色板 PNG，原样发送。
    pub(crate) fn bundled(bytes: &[u8], level: usize) -> Result<Self, String> {
        let width = *SIZES.get(level).ok_or("图片处理失败，请换一张试试")?;
        let preview = png_preview(bytes, width).map_err(|_| "图片处理失败，请换一张试试")?;
        Ok(Self {
            bytes: bytes.to_vec(),
            width,
            preview,
        })
    }
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
    // 索引 0 固定为全透明，其余 255 色由 NeuQuant 量化。
    let quant = NeuQuant::new(3, 255, rgba.as_raw());
    let mut palette = [[0u8; 4]; 256];
    for (i, entry) in palette.iter_mut().skip(1).enumerate() {
        *entry = quant.lookup(i).ok_or("图片处理失败，请换一张试试")?;
    }
    let pixels: Vec<u8> = rgba
        .pixels()
        .map(|pixel| {
            if pixel[3] == 0 {
                0
            } else {
                (quant.index_of(&pixel.0) + 1) as u8
            }
        })
        .collect();
    let bytes = encode_indexed(size, &palette, &pixels)?;
    let preview = data_url(&bytes);
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
                value["format"] = "png".into();
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
                Some("UNSUPPORTED_FORMAT") => "手环不支持这种图片格式，请重新连接后重试",
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

    fn noise(size: u32) -> Vec<u8> {
        let mut seed = 0x1234_5678u32;
        let image = RgbaImage::from_fn(size, size, |_, _| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let [a, b, c, _] = seed.to_le_bytes();
            Rgba([a, b, c, 255])
        });
        let mut output = Cursor::new(Vec::new());
        image.write_to(&mut output, ImageFormat::Png).unwrap();
        output.into_inner()
    }

    fn reply(read: &PreviewRead, bytes: &[u8], offset: usize) -> Value {
        let end = (offset + CHUNK_BYTES).min(bytes.len());
        json!({"v":1,"id":read.id,"type":"image-preview","level":read.level,"success":true,"format":"png",
            "width":SIZES[read.level],"length":bytes.len(),"offset":offset,
            "data":URL_SAFE_NO_PAD.encode(&bytes[offset..end])})
    }

    #[test]
    fn large_non_square_image_is_cropped_bounded_and_circular() {
        let result = convert(&encoded(900, 400), 10).unwrap();
        assert_eq!(result.width, 192);
        assert_eq!(result.bytes[..8], PNG_SIGNATURE);
        assert!(result.bytes.len() <= MAX_BYTES);
        let decoded = image::load_from_memory_with_format(&result.bytes, ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        assert_eq!(decoded.dimensions(), (192, 192));
        assert_eq!(decoded.get_pixel(0, 0)[3], 0, "圆外必须透明");
        let center = decoded.get_pixel(96, 96);
        assert!(center[0] > 220 && center[2] < 30 && center[3] > 240);
        assert_eq!(
            result.preview,
            data_url(&result.bytes),
            "预览与发送给手环的字节一致"
        );
    }

    #[test]
    fn png_within_limits_for_every_level() {
        for level in 0..SIZES.len() {
            let png = convert(&noise(400), level).unwrap();
            assert_eq!(png.width, SIZES[level]);
            assert!(
                png.bytes.len() <= MAX_BYTES,
                "level {level}: {} bytes",
                png.bytes.len()
            );
            assert_eq!(png.bytes[25], 3, "8 位调色板 PNG");
            assert!(png_preview(&png.bytes, png.width).is_ok());
        }
    }

    #[test]
    fn bundled_skins_are_valid_palette_png() {
        for skin in crate::skins::SKINS.iter() {
            for (level, bytes) in skin.fruits.iter().enumerate() {
                let image = Converted::bundled(bytes, level).unwrap();
                assert_eq!(image.width, SIZES[level]);
                assert!(image.bytes.len() <= MAX_BYTES);
                assert_eq!(
                    image.bytes[25],
                    3,
                    "{} fruit-{} 应为调色板 PNG",
                    skin.id,
                    level + 1
                );
            }
        }
        assert!(
            Converted::bundled(crate::skins::SKINS[0].fruits[0], 1).is_err(),
            "尺寸与等级不符必须拒绝"
        );
    }

    #[test]
    fn preview_reader_preserves_exact_bytes_and_rejects_wrong_chunks() {
        let image = convert(&noise(400), 10).unwrap();
        assert!(image.bytes.len() > CHUNK_BYTES);
        let mut reader = PreviewRead::new(10);
        let first = reply(&reader, &image.bytes, 0);
        assert_eq!(reader.accept(&first).unwrap(), Some(PreviewResult::Next));
        assert_eq!(reader.accept(&first).unwrap(), None, "重复分块忽略");
        for offset in (CHUNK_BYTES..image.bytes.len()).step_by(CHUNK_BYTES) {
            let expected = if offset + CHUNK_BYTES >= image.bytes.len() {
                PreviewResult::Complete
            } else {
                PreviewResult::Next
            };
            assert_eq!(
                reader
                    .accept(&reply(&reader, &image.bytes, offset))
                    .unwrap(),
                Some(expected)
            );
        }
        assert_eq!(reader.bytes, image.bytes);
        assert_eq!(reader.preview().unwrap(), image.preview);
        let mut invalid = PreviewRead::new(10);
        let mut skipped = reply(&invalid, &image.bytes, CHUNK_BYTES);
        skipped["offset"] = (CHUNK_BYTES * 2).into();
        assert!(invalid.accept(&skipped).is_err());
        assert!(invalid.bytes.is_empty());
    }

    #[test]
    fn bundled_previews_match_the_bytes_sent_to_the_band() {
        for (i, skin) in crate::skins::SKINS.iter().enumerate() {
            for (level, bytes) in skin.fruits.iter().enumerate() {
                assert_eq!(crate::skins::preview(i, level), data_url(bytes));
            }
        }
    }

    #[test]
    fn preview_rejects_legacy_bin_images() {
        // 旧版游戏回读 .bin 时不带 format，或带 format: "bin"。
        for format in [Value::Null, json!("bin")] {
            let mut reader = PreviewRead::new(0);
            let mut legacy = json!({"v":1,"id":reader.id,"type":"image-preview","level":0,"success":true,
                "width":36,"length":1036 + 36 * 36,"offset":0,"data":URL_SAFE_NO_PAD.encode([0u8; CHUNK_BYTES])});
            if !format.is_null() {
                legacy["format"] = format;
            }
            assert!(reader.accept(&legacy).is_err());
            assert!(reader.bytes.is_empty());
        }
    }
    #[test]
    fn reconnect_cache_loss_restarts_without_mixing_old_chunks() {
        let image = convert(&noise(400), 10).unwrap();
        let mut reader = PreviewRead::new(10);
        let first = reply(&reader, &image.bytes, 0);
        assert_eq!(reader.accept(&first).unwrap(), Some(PreviewResult::Next));
        let lost = json!({"v":1,"id":reader.id,"type":"image-preview","level":10,"success":false,"error":"NO_PREVIEW"});
        assert_eq!(reader.accept(&lost).unwrap(), Some(PreviewResult::Restart));
        reader.restart();
        assert_eq!(
            reader.accept(&first).unwrap(),
            None,
            "旧 id 的迟到分块不能混入"
        );
        for offset in (0..image.bytes.len()).step_by(CHUNK_BYTES) {
            let expected = if offset + CHUNK_BYTES >= image.bytes.len() {
                PreviewResult::Complete
            } else {
                PreviewResult::Next
            };
            assert_eq!(
                reader
                    .accept(&reply(&reader, &image.bytes, offset))
                    .unwrap(),
                Some(expected)
            );
        }
        assert_eq!(reader.preview().unwrap(), image.preview);
        reader.restart();
        assert_eq!(
            reader.accept(&reply(&reader, &image.bytes, 0)).unwrap(),
            Some(PreviewResult::Next)
        );
        let reset = json!({"v":1,"id":reader.id,"type":"image-preview","level":10,"success":true,"default":true});
        assert_eq!(
            reader.accept(&reset).unwrap(),
            Some(PreviewResult::Complete)
        );
        assert!(reader.bytes.is_empty(), "恢复默认的回执不能保留半张旧图片");
    }

    #[test]
    fn transfer_announces_png_and_only_advances_after_matching_offset() {
        let image = convert(&noise(400), 10).unwrap();
        let length = image.bytes.len();
        let mut t = Transfer::new("addr".into(), 10, Some(image));
        let begin: Value = serde_json::from_str(&t.packet()).unwrap();
        assert_eq!(begin["format"], "png");
        assert_eq!(begin["length"], length);
        assert_eq!(begin["width"], 192);
        let ok = json!({"v":1,"id":t.id,"type":"image-result","stage":"begin","success":true,"offset":0});
        assert_eq!(t.response(&ok).unwrap(), Some(false));
        assert_eq!(t.stage, Stage::Chunk);
        let wrong = json!({"v":1,"id":t.id,"type":"image-result","stage":"chunk","success":true,"offset":5});
        assert!(t.response(&wrong).is_err());
        assert_eq!(t.offset, 0);
        let mut t = Transfer::new("addr".into(), 10, Some(convert(&noise(400), 10).unwrap()));
        t.response(&json!({"v":1,"id":t.id,"type":"image-result","stage":"begin","success":true,"offset":0})).unwrap();
        let chunk = json!({"v":1,"id":t.id,"type":"image-result","stage":"chunk","success":true,"offset":CHUNK_BYTES});
        assert_eq!(t.response(&chunk).unwrap(), Some(false));
        assert_eq!(t.offset, CHUNK_BYTES);
        assert_eq!(t.response(&chunk).unwrap(), None);
        let reset = Transfer::new("addr".into(), 0, None);
        let packet: Value = serde_json::from_str(&reset.packet()).unwrap();
        assert_eq!(packet["type"], "image-reset");
        assert!(packet.get("format").is_none());
    }
}
