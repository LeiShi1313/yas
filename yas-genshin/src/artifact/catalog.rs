use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};
use flate2::read::GzDecoder;
use log::{info, warn};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ArtifactSetName, ArtifactSlot, EquippedCharacter};

const SCHEMA_VERSION: u32 = 1;
const CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_COMPRESSED_BYTES: usize = 2 * 1024 * 1024;
const MAX_DECOMPRESSED_BYTES: u64 = 16 * 1024 * 1024;
const DIST_BASE_URL: &str =
    "https://raw.githubusercontent.com/theBowja/genshin-db-dist/main/data/gzips";

const ARTIFACT_ZH_URL: &str =
    "https://raw.githubusercontent.com/theBowja/genshin-db-dist/main/data/gzips/chinesesimplified-artifacts.min.json.gzip";
const ARTIFACT_EN_URL: &str =
    "https://raw.githubusercontent.com/theBowja/genshin-db-dist/main/data/gzips/english-artifacts.min.json.gzip";
const CHARACTER_ZH_URL: &str =
    "https://raw.githubusercontent.com/theBowja/genshin-db-dist/main/data/gzips/chinesesimplified-characters.min.json.gzip";
const CHARACTER_EN_URL: &str =
    "https://raw.githubusercontent.com/theBowja/genshin-db-dist/main/data/gzips/english-characters.min.json.gzip";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSource {
    pub repository: String,
    pub commit: String,
    pub game_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactPieceRecord {
    pub name_zh_cn: String,
    pub slot: ArtifactSlot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSetRecord {
    pub id: u32,
    pub name_zh_cn: String,
    pub name_en: String,
    pub good_key: String,
    pub pieces: Vec<ArtifactPieceRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterRecord {
    pub id: u32,
    pub name_zh_cn: String,
    pub name_en: String,
    pub good_key: String,
    #[serde(default)]
    pub aliases_zh_cn: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactCatalog {
    pub schema_version: u32,
    pub source: CatalogSource,
    pub artifact_sets: Vec<ArtifactSetRecord>,
    pub characters: Vec<CharacterRecord>,
}

#[derive(Debug, Clone)]
pub struct ArtifactPieceMatch {
    pub set_name: ArtifactSetName,
    pub slot: ArtifactSlot,
    pub piece_name_zh_cn: String,
    pub distance: usize,
}

#[derive(Debug, Clone)]
pub struct CharacterMatch {
    pub character: EquippedCharacter,
    pub distance: usize,
}

impl ArtifactCatalog {
    pub fn from_json(content: &str) -> Result<Self> {
        let catalog: Self =
            serde_json::from_str(content).context("invalid artifact catalog JSON")?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn embedded() -> Result<Self> {
        Self::from_json(include_str!("artifact_catalog.json"))
    }

    pub fn load(path: Option<&Path>, update: bool) -> Result<Self> {
        if let Some(path) = path {
            return Self::from_path(path);
        }

        let cache_path = default_cache_path();
        let cached = cache_path
            .as_deref()
            .and_then(|path| Self::from_path(path).ok());
        let cache_is_fresh = cache_path
            .as_deref()
            .and_then(|path| fs::metadata(path).ok())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .map(|age| age < CACHE_MAX_AGE)
            .unwrap_or(false);

        if update && !cache_is_fresh {
            match Self::download_latest() {
                Ok(catalog) => {
                    if let Some(path) = cache_path.as_deref() {
                        if let Err(error) = catalog.write_to_path(path) {
                            warn!("无法缓存 genshin-db 数据: {error:#}");
                        }
                    }
                    info!(
                        "已更新圣遗物目录: {} 个套装，{} 名角色",
                        catalog.artifact_sets.len(),
                        catalog.characters.len()
                    );
                    return Ok(catalog);
                },
                Err(error) => warn!("更新 genshin-db 数据失败，使用本地目录: {error:#}"),
            }
        }

        if let Some(catalog) = cached {
            return Ok(catalog);
        }

        Self::embedded()
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read artifact catalog {}", path.display()))?;
        Self::from_json(&content)
    }

    pub fn write_to_path(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let temp_path = path.with_extension("json.tmp");
        let content = serde_json::to_vec_pretty(self)?;
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(&content)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(temp_path, path)?;
        Ok(())
    }

    pub fn find_piece(&self, candidates: &[&str]) -> Option<ArtifactPieceMatch> {
        self.find_piece_for_slot(candidates, None)
    }

    pub fn find_piece_in_slot(
        &self,
        candidates: &[&str],
        slot: &ArtifactSlot,
    ) -> Option<ArtifactPieceMatch> {
        self.find_piece_for_slot(candidates, Some(slot))
    }

    fn find_piece_for_slot(
        &self,
        candidates: &[&str],
        slot: Option<&ArtifactSlot>,
    ) -> Option<ArtifactPieceMatch> {
        let mut matches = Vec::new();
        for candidate in candidates {
            let candidate = normalize(candidate);
            if candidate.is_empty() {
                continue;
            }
            for artifact_set in &self.artifact_sets {
                for piece in &artifact_set.pieces {
                    if slot.is_some_and(|slot| piece.slot != *slot) {
                        continue;
                    }
                    let target = normalize(&piece.name_zh_cn);
                    matches.push((
                        char_edit_distance(&candidate, &target),
                        target.chars().count(),
                        artifact_set,
                        piece,
                    ));
                }
            }
        }

        matches.sort_by_key(|entry| entry.0);
        let best = matches.first()?;
        let second_distance = matches
            .iter()
            .find(|entry| entry.2.id != best.2.id || entry.3.name_zh_cn != best.3.name_zh_cn)
            .map(|entry| entry.0);

        if best.0 > allowed_piece_distance(best.1)
            || second_distance.is_some_and(|distance| distance <= best.0)
        {
            return None;
        }

        Some(ArtifactPieceMatch {
            set_name: ArtifactSetName::from_record(best.2),
            slot: best.3.slot.clone(),
            piece_name_zh_cn: best.3.name_zh_cn.clone(),
            distance: best.0,
        })
    }

    pub fn find_character(&self, candidates: &[&str]) -> Option<CharacterMatch> {
        let mut matches = Vec::new();
        for candidate in candidates {
            let candidate = normalize(candidate);
            if candidate.is_empty() {
                continue;
            }
            for character in &self.characters {
                for name in std::iter::once(&character.name_zh_cn).chain(&character.aliases_zh_cn) {
                    for target in [name.clone(), format!("{name}已装备")] {
                        matches.push((
                            char_edit_distance(&candidate, &normalize(&target)),
                            target.chars().count(),
                            character,
                        ));
                    }
                }
            }
        }

        matches.sort_by_key(|entry| entry.0);
        let best = matches.first()?;
        let second_distance = matches
            .iter()
            .find(|entry| entry.2.id != best.2.id)
            .map(|entry| entry.0);

        if best.0 > allowed_character_distance(best.1)
            || second_distance.is_some_and(|distance| distance <= best.0)
        {
            return None;
        }

        Some(CharacterMatch {
            character: EquippedCharacter::from_record(best.2),
            distance: best.0,
        })
    }

    pub fn download_latest() -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("yas-artifact-catalog/1")
            .build()?;

        let artifact_zh = download_gzip_json(&client, ARTIFACT_ZH_URL)?;
        let artifact_en = download_gzip_json(&client, ARTIFACT_EN_URL)?;
        let character_zh = download_gzip_json(&client, CHARACTER_ZH_URL)?;
        let character_en = download_gzip_json(&client, CHARACTER_EN_URL)?;

        let artifact_sets = build_artifact_sets(&artifact_zh, &artifact_en)?;
        let characters = build_characters(&character_zh, &character_en)?;
        let catalog = Self {
            schema_version: SCHEMA_VERSION,
            source: CatalogSource {
                repository: DIST_BASE_URL.to_string(),
                commit: "main".to_string(),
                game_version: "latest".to_string(),
            },
            artifact_sets,
            characters,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(anyhow!(
                "unsupported artifact catalog schema {}",
                self.schema_version
            ));
        }
        if self.artifact_sets.is_empty() || self.characters.is_empty() {
            return Err(anyhow!("artifact catalog is empty"));
        }

        let mut set_ids = HashSet::new();
        let mut piece_names = HashSet::new();
        for artifact_set in &self.artifact_sets {
            if !set_ids.insert(artifact_set.id) {
                return Err(anyhow!("duplicate artifact set id {}", artifact_set.id));
            }
            if artifact_set.good_key.is_empty()
                || !artifact_set
                    .good_key
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric())
            {
                return Err(anyhow!("invalid GOOD key {}", artifact_set.good_key));
            }
            for piece in &artifact_set.pieces {
                if !piece_names.insert(piece.name_zh_cn.as_str()) {
                    return Err(anyhow!("duplicate artifact piece {}", piece.name_zh_cn));
                }
            }
        }
        Ok(())
    }
}

fn default_cache_path() -> Option<PathBuf> {
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        return Some(
            PathBuf::from(local_app_data)
                .join("yas")
                .join("genshin_catalog.json"),
        );
    }
    if let Some(cache_home) = std::env::var_os("XDG_CACHE_HOME") {
        return Some(
            PathBuf::from(cache_home)
                .join("yas")
                .join("genshin_catalog.json"),
        );
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".cache").join("yas").join("genshin_catalog.json"))
}

fn download_gzip_json(client: &Client, url: &str) -> Result<Value> {
    let response = client.get(url).send()?.error_for_status()?;
    let compressed = response.bytes()?;
    if compressed.len() > MAX_COMPRESSED_BYTES {
        return Err(anyhow!("genshin-db response is unexpectedly large"));
    }

    let decoder = GzDecoder::new(compressed.as_ref());
    let mut content = String::new();
    decoder
        .take(MAX_DECOMPRESSED_BYTES)
        .read_to_string(&mut content)?;
    serde_json::from_str(&content).context("invalid genshin-db distribution JSON")
}

fn language_folder<'a>(
    root: &'a Value,
    language: &str,
    folder: &str,
) -> Result<&'a serde_json::Map<String, Value>> {
    root.get("data")
        .and_then(|value| value.get(language))
        .and_then(|value| value.get(folder))
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("missing genshin-db {language}/{folder} data"))
}

fn build_artifact_sets(zh_root: &Value, en_root: &Value) -> Result<Vec<ArtifactSetRecord>> {
    let zh = language_folder(zh_root, "ChineseSimplified", "artifacts")?;
    let en = language_folder(en_root, "English", "artifacts")?;
    let slots = [
        ("flower", ArtifactSlot::Flower),
        ("plume", ArtifactSlot::Feather),
        ("sands", ArtifactSlot::Sand),
        ("goblet", ArtifactSlot::Goblet),
        ("circlet", ArtifactSlot::Head),
    ];
    let mut result = Vec::new();

    for (key, zh_set) in zh {
        let en_set = en
            .get(key)
            .ok_or_else(|| anyhow!("missing English artifact {key}"))?;
        let id = value_u32(zh_set, "id")?;
        let name_zh_cn = value_string(zh_set, "name")?;
        let name_en = value_string(en_set, "name")?;
        let mut pieces = Vec::new();
        for (field, slot) in &slots {
            if let Some(piece) = zh_set.get(field) {
                pieces.push(ArtifactPieceRecord {
                    name_zh_cn: value_string(piece, "name")?,
                    slot: slot.clone(),
                });
            }
        }
        result.push(ArtifactSetRecord {
            id,
            name_zh_cn,
            good_key: pascal_key(&name_en),
            name_en,
            pieces,
        });
    }
    result.sort_by_key(|item| item.id);
    Ok(result)
}

fn build_characters(zh_root: &Value, en_root: &Value) -> Result<Vec<CharacterRecord>> {
    let zh = language_folder(zh_root, "ChineseSimplified", "characters")?;
    let en = language_folder(en_root, "English", "characters")?;
    let mut result = vec![CharacterRecord {
        id: 0,
        name_zh_cn: "旅行者".to_string(),
        name_en: "Traveler".to_string(),
        good_key: "Traveler".to_string(),
        aliases_zh_cn: vec!["空".to_string(), "荧".to_string()],
    }];

    for (key, zh_character) in zh {
        if key == "aether" || key == "lumine" {
            continue;
        }
        let en_character = en
            .get(key)
            .ok_or_else(|| anyhow!("missing English character {key}"))?;
        let name_en = value_string(en_character, "name")?;
        result.push(CharacterRecord {
            id: value_u32(zh_character, "id")?,
            name_zh_cn: value_string(zh_character, "name")?,
            good_key: pascal_key(&name_en),
            name_en,
            aliases_zh_cn: Vec::new(),
        });
    }
    result.sort_by_key(|item| item.id);
    Ok(result)
}

fn value_string(value: &Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing string field {field}"))
}

fn value_u32(value: &Value, field: &str) -> Result<u32> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| anyhow!("missing integer field {field}"))
}

fn pascal_key(value: &str) -> String {
    let mut result = String::new();
    let mut uppercase_next = true;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if uppercase_next {
                result.extend(ch.to_uppercase());
                uppercase_next = false;
            } else {
                result.push(ch);
            }
        } else if ch != '\'' && ch != '’' {
            uppercase_next = true;
        }
    }
    result
}

fn normalize(value: &str) -> String {
    value.chars().filter(|ch| ch.is_alphanumeric()).collect()
}

fn char_edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0; right.len() + 1];

    for (left_index, left_char) in left.chars().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right.iter().enumerate() {
            let substitution = previous[right_index] + usize::from(left_char != *right_char);
            current[right_index + 1] = (current[right_index] + 1)
                .min(previous[right_index + 1] + 1)
                .min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

fn allowed_piece_distance(length: usize) -> usize {
    match length {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    }
}

fn allowed_character_distance(length: usize) -> usize {
    match length {
        0..=2 => 0,
        3..=6 => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_contains_genshin_6_7_sets() {
        let catalog = ArtifactCatalog::embedded().unwrap();
        assert!(catalog.artifact_sets.len() >= 61);
        let matched = catalog.find_piece(&["风花的箴铭"]).unwrap();
        assert_eq!(matched.set_name.good_key(), "ADayCarvedFromRisingWinds");
        assert_eq!(matched.slot, ArtifactSlot::Flower);
    }

    #[test]
    fn catalog_recovers_one_missing_rare_character() {
        let catalog = ArtifactCatalog::embedded().unwrap();
        let matched = catalog.find_piece(&["献与月的祭"]).unwrap();
        assert_eq!(matched.piece_name_zh_cn, "献与月的酹祭");
        assert_eq!(matched.distance, 1);
    }

    #[test]
    fn slot_constraint_disambiguates_shared_piece_prefix() {
        let catalog = ArtifactCatalog::embedded().unwrap();
        let matched = catalog
            .find_piece_in_slot(&["天授之"], &ArtifactSlot::Feather)
            .unwrap();
        assert_eq!(matched.piece_name_zh_cn, "天授之殁");
    }

    #[test]
    fn catalog_matches_equipped_character_text() {
        let catalog = ArtifactCatalog::embedded().unwrap();
        let matched = catalog.find_character(&["杜林已装备"]).unwrap();
        assert_eq!(matched.character.good_key(), "Durin");
    }

    #[test]
    fn pascal_key_matches_good_conventions() {
        assert_eq!(pascal_key("Gladiator's Finale"), "GladiatorsFinale");
        assert_eq!(
            pascal_key("Night of the Sky's Unveiling"),
            "NightOfTheSkysUnveiling"
        );
    }

    #[test]
    #[ignore = "requires network access"]
    fn downloads_current_genshin_db_distribution() {
        let catalog = ArtifactCatalog::download_latest().unwrap();
        assert!(catalog.artifact_sets.len() >= 61);
        assert!(catalog.characters.len() >= 100);
        assert!(catalog.find_piece(&["风花的箴铭"]).is_some());
    }
}
