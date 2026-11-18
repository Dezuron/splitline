//! Domain lists (`geosite.dat`): read, merge and write.
//!
//! Xray matches `geosite:<category>` against a protobuf file in its resource
//! directory. The app builds that file itself from community sources, so a site
//! can be routed as one category instead of a pile of CDN domains.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Xray's `Domain.Type`: 0 plain, 1 regexp, 2 domain (subdomains too), 3 full.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Domain {
    pub kind: i32,
    pub value: String,
}

#[derive(Debug, Clone, Default)]
pub struct Site {
    pub code: String,
    pub domains: Vec<Domain>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryInfo {
    pub code: String,
    pub domains: usize,
}

/// Small sidecar written next to `geosite.dat`: what the UI can offer and what
/// counts as a valid category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoIndex {
    pub updated_at: u64,
    pub categories: Vec<CategoryInfo>,
}

impl GeoIndex {
    pub fn get(&self, code: &str) -> Option<&CategoryInfo> {
        self.categories
            .iter()
            .find(|category| category.code.eq_ignore_ascii_case(code))
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub fn read(path: &Path) -> Result<Vec<Site>, String> {
    let data =
        fs::read(path).map_err(|e| format!("Не удалось прочитать {}: {e}", path.display()))?;
    Ok(parse(&data))
}

/// Writes through a temporary file so a failed update never leaves a broken
/// `geosite.dat` behind — Xray refuses to start on one.
pub fn write(path: &Path, sites: &[Site]) -> Result<(), String> {
    let bytes = encode(sites);
    let temp = path.with_extension("tmp");
    fs::write(&temp, &bytes).map_err(|e| format!("Не удалось записать базы: {e}"))?;
    fs::rename(&temp, path).map_err(|e| format!("Не удалось заменить базы: {e}"))?;
    Ok(())
}

/// Unions categories from several sources and drops duplicate domains.
pub fn merge(sources: Vec<Vec<Site>>) -> Vec<Site> {
    let mut map: BTreeMap<String, BTreeSet<Domain>> = BTreeMap::new();
    for sites in sources {
        for site in sites {
            if site.code.is_empty() {
                continue;
            }
            map.entry(site.code).or_default().extend(site.domains);
        }
    }
    map.into_iter()
        .map(|(code, domains)| Site {
            code,
            domains: domains.into_iter().collect(),
        })
        .collect()
}

pub fn index(sites: &[Site]) -> GeoIndex {
    GeoIndex {
        updated_at: now_secs(),
        categories: sites
            .iter()
            .map(|site| CategoryInfo {
                code: site.code.clone(),
                domains: site.domains.len(),
            })
            .collect(),
    }
}

// ---------- protobuf ----------

fn parse(data: &[u8]) -> Vec<Site> {
    let mut sites = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let Some(key) = varint(data, &mut pos) else {
            break;
        };
        let field = (key >> 3) as u32;
        let wire = (key & 7) as u32;
        let Some(slice) = take(data, &mut pos, wire) else {
            break;
        };
        if field == 1 && wire == 2 {
            if let Some(site) = parse_site(slice) {
                sites.push(site);
            }
        }
    }
    sites
}

fn parse_site(data: &[u8]) -> Option<Site> {
    let mut site = Site::default();
    let mut pos = 0usize;
    while pos < data.len() {
        let key = varint(data, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 7) as u32;
        let slice = take(data, &mut pos, wire)?;
        match (field, wire) {
            (1, 2) => site.code = String::from_utf8_lossy(slice).into_owned(),
            (2, 2) => {
                if let Some(domain) = parse_domain(slice) {
                    site.domains.push(domain);
                }
            }
            _ => {}
        }
    }
    Some(site)
}

fn parse_domain(data: &[u8]) -> Option<Domain> {
    let mut domain = Domain {
        kind: 0,
        value: String::new(),
    };
    let mut pos = 0usize;
    while pos < data.len() {
        let key = varint(data, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 7) as u32;
        let slice = take(data, &mut pos, wire)?;
        match (field, wire) {
            (1, 0) => {
                let mut inner = 0usize;
                domain.kind = varint(slice, &mut inner).unwrap_or(0) as i32;
            }
            (2, 2) => domain.value = String::from_utf8_lossy(slice).into_owned(),
            _ => {}
        }
    }
    (!domain.value.is_empty()).then_some(domain)
}

fn encode(sites: &[Site]) -> Vec<u8> {
    let mut out = Vec::new();
    for site in sites {
        let mut body = Vec::new();
        put_string(&mut body, 1, &site.code);
        for domain in &site.domains {
            let mut encoded = Vec::new();
            put_varint_field(&mut encoded, 1, domain.kind.max(0) as u64);
            put_string(&mut encoded, 2, &domain.value);
            put_bytes(&mut body, 2, &encoded);
        }
        put_bytes(&mut out, 1, &body);
    }
    out
}

fn put_varint_field(out: &mut Vec<u8>, field: u32, value: u64) {
    put_varint(out, (field as u64) << 3);
    put_varint(out, value);
}

fn put_string(out: &mut Vec<u8>, field: u32, value: &str) {
    put_bytes(out, field, value.as_bytes());
}

fn put_bytes(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    put_varint(out, ((field as u64) << 3) | 2);
    put_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Returns the payload of one field: a varint value or a length-delimited slice.
fn take<'a>(data: &'a [u8], pos: &mut usize, wire: u32) -> Option<&'a [u8]> {
    match wire {
        0 => {
            let start = *pos;
            varint(data, pos)?;
            Some(&data[start..*pos])
        }
        2 => {
            let length = varint(data, pos)? as usize;
            let start = *pos;
            *pos += length;
            (start + length <= data.len()).then(|| &data[start..start + length])
        }
        1 => {
            let start = *pos;
            *pos += 8;
            (start + 8 <= data.len()).then(|| &data[start..start + 8])
        }
        5 => {
            let start = *pos;
            *pos += 4;
            (start + 4 <= data.len()).then(|| &data[start..start + 4])
        }
        _ => None,
    }
}

fn varint(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;
    while *pos < data.len() {
        let byte = data[*pos];
        *pos += 1;
        value |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(code: &str, domains: &[&str]) -> Site {
        Site {
            code: code.into(),
            domains: domains
                .iter()
                .map(|value| Domain {
                    kind: 2,
                    value: (*value).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn merged_categories_keep_one_copy_of_each_domain() {
        let merged = merge(vec![
            vec![site("meta", &["instagram.com", "fbcdn.net"])],
            vec![site("meta", &["fbcdn.net", "threads.net"])],
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].domains.len(), 3);
    }

    #[test]
    fn encoding_round_trips_through_the_parser() {
        let original = vec![site("discord", &["discord.com", "discord.gg"])];
        let decoded = parse(&encode(&original));
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].code, "discord");
        assert_eq!(decoded[0].domains.len(), 2);
        assert_eq!(decoded[0].domains[1].value, "discord.gg");
        assert_eq!(decoded[0].domains[1].kind, 2);
    }

    #[test]
    fn index_reports_category_sizes() {
        let index = index(&[site("meta", &["a.com", "b.com"]), site("bbc", &["bbc.com"])]);
        assert_eq!(index.get("meta").map(|c| c.domains), Some(2));
        assert!(index.get("nope").is_none());
    }

    /// Runs on the real downloaded lists: `GEODAT_A`, `GEODAT_B` in, `GEODAT_OUT`
    /// out. Ignored by default because it needs those files on disk.
    #[test]
    #[ignore]
    fn merge_real_lists() {
        let first = std::env::var("GEODAT_A").expect("GEODAT_A");
        let second = std::env::var("GEODAT_B").expect("GEODAT_B");
        let out = std::env::var("GEODAT_OUT").expect("GEODAT_OUT");
        let sites = merge(vec![
            read(Path::new(&first)).expect("first list"),
            read(Path::new(&second)).expect("second list"),
        ]);
        write(Path::new(&out), &sites).expect("write merged");
        let disk = read(Path::new(&out)).expect("read back");
        assert_eq!(disk.len(), sites.len());
        assert!(disk.len() > 1000, "expected the full category set");
    }
}
