//! WOFF2 → SFNT at registration. allsorts reconstructs transformed `glyf` /
//! `loca` / `hmtx`; we write those tables back as a single-face OpenType file
//! because the crate has no decoder-only emit.

use allsorts::binary::read::ReadScope;
use allsorts::font_data::FontData as AllsortsFontData;
use allsorts::tables::{FontTableProvider, SfntVersion};

pub(crate) fn to_sfnt(bytes: &[u8], face_index: u32) -> Option<Vec<u8>> {
    let font = ReadScope::new(bytes)
        .read::<AllsortsFontData<'_>>()
        .ok()?;
    let provider = font.table_provider(face_index as usize).ok()?;
    let flavor = provider.sfnt_version();
    let tags = provider.table_tags()?;
    let mut tables = Vec::with_capacity(tags.len());
    for tag in tags {
        let data = provider.read_table_data(tag).ok()?;
        tables.push((tag, data.into_owned()));
    }
    Some(write_sfnt(flavor, &tables))
}

fn write_sfnt(flavor: u32, tables: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut records: Vec<(u32, u32, u32, Vec<u8>)> = tables
        .iter()
        .map(|(tag, bytes)| {
            let mut padded = bytes.clone();
            while padded.len() % 4 != 0 {
                padded.push(0);
            }
            let mut sum = 0u32;
            for chunk in padded.chunks_exact(4) {
                let mut word = [0u8; 4];
                word.copy_from_slice(chunk);
                sum = sum.wrapping_add(u32::from_be_bytes(word));
            }
            (*tag, sum, bytes.len() as u32, padded)
        })
        .collect();
    records.sort_by_key(|record| record.0);

    let n = records.len() as u16;
    let entry = 15u16.saturating_sub(n.leading_zeros() as u16);
    let search = (1u16 << entry) * 16;
    let dir_end = 12 + records.len() * 16;

    let mut out = Vec::with_capacity(dir_end + records.iter().map(|r| r.3.len()).sum::<usize>());
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(&search.to_be_bytes());
    out.extend_from_slice(&entry.to_be_bytes());
    out.extend_from_slice(&(n * 16 - search).to_be_bytes());

    let mut offset = dir_end as u32;
    for (tag, sum, length, padded) in &records {
        out.extend_from_slice(&tag.to_be_bytes());
        out.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&offset.to_be_bytes());
        out.extend_from_slice(&length.to_be_bytes());
        offset += padded.len() as u32;
    }
    for (_, _, _, padded) in records {
        out.extend_from_slice(&padded);
    }
    out
}
