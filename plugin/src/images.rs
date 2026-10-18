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

/// 手环能显示的图片格式。小米手环 9/9 Pro 不支持 LVGL v9 `.bin`，握手时报告 `png`；
/// 旧版游戏不带该字段，按 `.bin` 处理。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Format {
    #[default]
    Bin,
    Png,
}
impl Format {
    pub(crate) fn from_value(value: &Value) -> Self {
        if value == "png" { Self::Png } else { Self::Bin }
    }
}

/// 把 LVGL v9 I8 原样转成 8 位调色板 PNG：像素与调色板不变，手环上看到的和 `.bin` 一致。
pub(crate) fn indexed_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let width = indexed_width(bytes).ok_or("图片处理失败，请换一张试试")?;
    let mut palette = Vec::with_capacity(768);
    let mut alpha = Vec::with_capacity(256);
    for color in bytes[12..1036].chunks_exact(4) {
        palette.extend_from_slice(&[color[2], color[1], color[0]]);
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
        encoder.set_palette(palette);
        if !alpha.is_empty() {
            encoder.set_trns(alpha);
        }
        encoder.set_compression(png::Compression::High);
        let mut writer = encoder
            .write_header()
            .map_err(|_| "图片处理失败，请换一张试试")?;
        writer
            .write_image_data(&bytes[1036..])
            .map_err(|_| "图片处理失败，请换一张试试")?;
        writer.finish().map_err(|_| "图片处理失败，请换一张试试")?;
    }
    if output.len() > MAX_BYTES {
        return Err("图片处理失败，请换一张试试".into());
    }
    Ok(output)
}

/// 手环回读的 PNG：校验签名、IHDR 尺寸并能完整解码后才作为预览显示。
pub(crate) fn png_preview(bytes: &[u8], width: u16) -> Result<String, String> {
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
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

fn indexed_width(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 1037
        || bytes.len() > MAX_BYTES
        || bytes[..4] != [0x19, 0x0a, 0, 0]
        || bytes[10..12] != [0, 0]
    {
        return None;
    }
    let width = u16::from_le_bytes([bytes[4], bytes[5]]);
    (width > 0
        && width <= 192
        && u16::from_le_bytes([bytes[6], bytes[7]]) == width
        && u16::from_le_bytes([bytes[8], bytes[9]]) == width
        && bytes.len() == 1036 + usize::from(width) * usize::from(width))
    .then_some(width)
}

pub(crate) fn indexed_preview(bytes: &[u8]) -> Result<String, String> {
    let width = indexed_width(bytes).ok_or("图片预览加载失败")?;
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
    format: Format,
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
            format: Format::Bin,
        }
    }
    /// 回读完成后的预览：PNG 直接显示，`.bin` 按调色板解码。
    pub(crate) fn preview(&self) -> Result<String, String> {
        match self.format {
            Format::Png => png_preview(&self.bytes, self.width),
            Format::Bin => indexed_preview(&self.bytes),
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
        self.format = Format::Bin;
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
        let format = Format::from_value(&value["format"]);
        let valid_length = match format {
            Format::Png => length >= 33,
            Format::Bin => length == 1036 + usize::from(width) * usize::from(width),
        };
        if offset != self.bytes.len()
            || !valid_length
            || (self.length != 0
                && (length != self.length || width != self.width || format != self.format))
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
            self.format = format;
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
    pub(crate) format: Format,
}
impl Converted {
    /// 内置皮肤与自定义图片统一先生成 `.bin`，PNG 设备再无损转成调色板 PNG。
    pub(crate) fn indexed(bytes: Vec<u8>, preview: String, format: Format) -> Result<Self, String> {
        let width = indexed_width(&bytes).ok_or("图片处理失败，请换一张试试")?;
        let bytes = match format {
            Format::Bin => bytes,
            Format::Png => indexed_png(&bytes)?,
        };
        Ok(Self {
            bytes,
            width,
            preview,
            format,
        })
    }
}

pub(crate) fn convert(input: &[u8], level: usize, format: Format) -> Result<Converted, String> {
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
    Converted::indexed(bytes, preview, format)
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
    format: Format,
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
        let (bytes, width, format, stage) = match image {
            Some(image) => (image.bytes, image.width, image.format, Stage::Begin),
            None => (Vec::new(), 0, Format::Bin, Stage::Reset),
        };
        let crc32 = format!("{:08x}", crc32fast::hash(&bytes));
        Self {
            id: Uuid::new_v4().to_string(),
            addr,
            level,
            bytes,
            width,
            format,
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
                if self.format == Format::Png {
                    value["format"] = "png".into();
                }
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
    #[test]
    fn large_non_square_image_is_cropped_bounded_and_circular() {
        let result = convert(&encoded(900, 400), 10, Format::Bin).unwrap();
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
        let image = convert(&encoded(80, 100), 0, Format::Bin).unwrap();
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
        let image = convert(&encoded(60, 50), 0, Format::Bin).unwrap();
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
            Some(convert(&encoded(10, 10), 0, Format::Bin).unwrap()),
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

    #[test]
    fn png_devices_receive_lossless_palette_png_within_limits() {
        for level in [0, 10] {
            let bin = convert(&noise(400), level, Format::Bin).unwrap();
            let png = convert(&noise(400), level, Format::Png).unwrap();
            assert_eq!(png.format, Format::Png);
            assert_eq!(png.width, SIZES[level]);
            assert!(png.bytes.len() <= MAX_BYTES, "{} bytes", png.bytes.len());
            assert_eq!(png.bytes[..8], PNG_SIGNATURE);
            let decoded = image::load_from_memory(&png.bytes).unwrap().into_rgba8();
            let expected = base64::engine::general_purpose::STANDARD
                .decode(bin.preview.split(',').nth(1).unwrap())
                .unwrap();
            let expected = image::load_from_memory(&expected).unwrap().into_rgba8();
            assert_eq!(decoded, expected, "PNG 必须与 .bin 像素一致");
        }
    }

    #[test]
    fn bundled_skins_convert_to_png() {
        for skin in crate::skins::SKINS.iter() {
            for (level, bytes) in skin.fruits.iter().enumerate() {
                let image = Converted::indexed(bytes.to_vec(), String::new(), Format::Png).unwrap();
                assert_eq!(image.width, SIZES[level]);
                assert!(png_preview(&image.bytes, image.width).is_ok());
            }
        }
    }

    #[test]
    fn png_transfer_announces_format_and_preview_reads_png() {
        let image = convert(&encoded(50, 50), 0, Format::Png).unwrap();
        let bytes = image.bytes.clone();
        let t = Transfer::new("addr".into(), 0, Some(image));
        let begin: Value = serde_json::from_str(&t.packet()).unwrap();
        assert_eq!(begin["format"], "png");
        assert_eq!(begin["length"], bytes.len());
        let bin = Transfer::new(
            "addr".into(),
            0,
            Some(convert(&encoded(50, 50), 0, Format::Bin).unwrap()),
        );
        let begin: Value = serde_json::from_str(&bin.packet()).unwrap();
        assert!(
            begin.get("format").is_none(),
            "旧版游戏只认识不带 format 的 .bin"
        );
        let mut reader = PreviewRead::new(0);
        let reply = json!({"v":1,"id":reader.id,"type":"image-preview","level":0,"success":true,"format":"png",
            "width":36,"length":bytes.len(),"offset":0,"data":URL_SAFE_NO_PAD.encode(&bytes)});
        assert_eq!(
            reader.accept(&reply).unwrap(),
            Some(PreviewResult::Complete)
        );
        assert!(
            reader
                .preview()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
    }
}
