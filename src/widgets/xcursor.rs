pub(crate) struct DecodedCursor {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

const MAGIC: &[u8; 4] = b"Xcur";
const IMAGE_TYPE: u32 = 0xfffd0002;

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

pub(crate) fn decode(data: &[u8]) -> Option<DecodedCursor> {
    if data.get(..4)? != MAGIC {
        return None;
    }
    let header = read_u32(data, 4)? as usize;
    let count = read_u32(data, 12)? as usize;
    if header < 16 || count == 0 || header + count * 12 > data.len() {
        return None;
    }
    let mut best: Option<(u32, u64, usize, u32, u32)> = None;
    for index in 0..count {
        let toc = header + index * 12;
        let kind = read_u32(data, toc)?;
        if kind != IMAGE_TYPE {
            continue;
        }
        let position = read_u32(data, toc + 8)? as usize;
        let chunk_header = read_u32(data, position)? as usize;
        let chunk_kind = read_u32(data, position + 4)?;
        let nominal = read_u32(data, position + 8)?;
        let width = read_u32(data, position + 16)?;
        let height = read_u32(data, position + 20)?;
        if chunk_kind != IMAGE_TYPE || chunk_header < 36 || width == 0 || height == 0 {
            continue;
        }
        let pixels = position.checked_add(chunk_header)?;
        let area = u64::from(width) * u64::from(height);
        if pixels.checked_add(area as usize * 4)? > data.len() {
            continue;
        }
        let rank = if nominal <= 48 {
            (48 - nominal, area)
        } else {
            (nominal - 48, area)
        };
        let candidate = (rank.0, rank.1, pixels, width, height);
        if best.as_ref().is_none_or(|current| candidate.0 < current.0) {
            best = Some(candidate);
        }
    }
    let (_, _, start, width, height) = best?;
    let count = width as usize * height as usize;
    let mut rgba = Vec::with_capacity(count * 4);
    for index in 0..count {
        let pixel = read_u32(data, start + index * 4)?;
        rgba.push(((pixel >> 16) & 0xff) as u8);
        rgba.push(((pixel >> 8) & 0xff) as u8);
        rgba.push((pixel & 0xff) as u8);
        rgba.push(((pixel >> 24) & 0xff) as u8);
    }
    Some(DecodedCursor {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor_file(nominal: u32, width: u32, height: u32, pixel: u32) -> Vec<u8> {
        let header = 16u32;
        let image = header + 12;
        let chunk = 36u32;
        let mut data = Vec::new();
        data.extend_from_slice(MAGIC);
        data.extend_from_slice(&header.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&IMAGE_TYPE.to_le_bytes());
        data.extend_from_slice(&nominal.to_le_bytes());
        data.extend_from_slice(&image.to_le_bytes());
        data.extend_from_slice(&chunk.to_le_bytes());
        data.extend_from_slice(&IMAGE_TYPE.to_le_bytes());
        data.extend_from_slice(&nominal.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&width.to_le_bytes());
        data.extend_from_slice(&height.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        for _ in 0..width * height {
            data.extend_from_slice(&pixel.to_le_bytes());
        }
        data
    }

    #[test]
    fn decodes_argb_to_rgba() {
        let decoded = decode(&cursor_file(24, 1, 1, 0x80102030)).unwrap();
        assert_eq!(decoded.width, 1);
        assert_eq!(decoded.height, 1);
        assert_eq!(decoded.rgba, vec![0x10, 0x20, 0x30, 0x80]);
    }

    #[test]
    fn rejects_bad_magic_and_truncated_files() {
        assert!(decode(b"nope").is_none());
        assert!(decode(b"Xcur").is_none());
        let mut data = cursor_file(24, 1, 1, 0);
        data.truncate(data.len() - 2);
        assert!(decode(&data).is_none());
    }

    #[test]
    fn prefers_the_largest_size_up_to_the_cap() {
        let header = 16u32;
        let count = 2u32;
        let first = header + count * 12;
        let first_size = 36u32 + 24 * 24 * 4;
        let second = first as usize + first_size as usize;
        let mut data = Vec::new();
        data.extend_from_slice(MAGIC);
        data.extend_from_slice(&header.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&count.to_le_bytes());
        for (nominal, position) in [(16u32, first), (32u32, second as u32)] {
            data.extend_from_slice(&IMAGE_TYPE.to_le_bytes());
            data.extend_from_slice(&nominal.to_le_bytes());
            data.extend_from_slice(&position.to_le_bytes());
        }
        for nominal in [16u32, 32u32] {
            data.extend_from_slice(&36u32.to_le_bytes());
            data.extend_from_slice(&IMAGE_TYPE.to_le_bytes());
            data.extend_from_slice(&nominal.to_le_bytes());
            data.extend_from_slice(&1u32.to_le_bytes());
            data.extend_from_slice(&24u32.to_le_bytes());
            data.extend_from_slice(&24u32.to_le_bytes());
            data.extend_from_slice(&0u32.to_le_bytes());
            data.extend_from_slice(&0u32.to_le_bytes());
            data.extend_from_slice(&0u32.to_le_bytes());
            data.extend(std::iter::repeat_n(0u8, 24 * 24 * 4));
        }
        let decoded = decode(&data).unwrap();
        assert_eq!(decoded.width.max(decoded.height), 24);
        assert_eq!(decoded.rgba.len(), 24 * 24 * 4);
    }
}
