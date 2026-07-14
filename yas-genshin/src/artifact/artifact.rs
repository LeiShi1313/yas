use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};

use log::error;
use regex::Regex;
use serde::{Deserialize, Serialize};
use strum_macros::Display;

use crate::scanner::GenshinArtifactScanResult;

use super::catalog::{ArtifactCatalog, ArtifactSetRecord, CharacterRecord};

#[derive(Debug, Hash, Clone, PartialEq, Eq, Display)]
pub enum ArtifactStatName {
    HealingBonus,
    CriticalDamage,
    Critical,
    Atk,
    AtkPercentage,
    ElementalMastery,
    Recharge,
    HpPercentage,
    Hp,
    DefPercentage,
    Def,
    ElectroBonus,
    PyroBonus,
    HydroBonus,
    CryoBonus,
    AnemoBonus,
    GeoBonus,
    PhysicalBonus,
    DendroBonus,
}

#[derive(Debug, Hash, Copy, Clone, PartialEq, Eq, Display, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactSlot {
    Flower,
    Feather,
    Sand,
    Goblet,
    Head,
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
pub struct ArtifactSetName {
    id: u32,
    name_zh_cn: String,
    name_en: String,
    good_key: String,
}

impl ArtifactSetName {
    pub(crate) fn from_record(record: &ArtifactSetRecord) -> Self {
        Self {
            id: record.id,
            name_zh_cn: record.name_zh_cn.clone(),
            name_en: record.name_en.clone(),
            good_key: record.good_key.clone(),
        }
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn name_zh_cn(&self) -> &str {
        &self.name_zh_cn
    }

    pub fn name_en(&self) -> &str {
        &self.name_en
    }

    pub fn good_key(&self) -> &str {
        &self.good_key
    }
}

impl Display for ArtifactSetName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.good_key)
    }
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
pub struct EquippedCharacter {
    id: u32,
    name_zh_cn: String,
    good_key: String,
}

impl EquippedCharacter {
    pub(crate) fn from_record(record: &CharacterRecord) -> Self {
        Self {
            id: record.id,
            name_zh_cn: record.name_zh_cn.clone(),
            good_key: record.good_key.clone(),
        }
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn name_zh_cn(&self) -> &str {
        &self.name_zh_cn
    }

    pub fn good_key(&self) -> &str {
        &self.good_key
    }
}

#[derive(Debug, Clone)]
pub struct ArtifactStat {
    pub name: ArtifactStatName,
    pub value: f64,
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
pub struct GenshinArtifact {
    pub set_name: ArtifactSetName,
    pub slot: ArtifactSlot,
    pub star: i32,
    pub lock: bool,
    pub level: i32,
    pub main_stat: ArtifactStat,
    pub sub_stat_1: Option<ArtifactStat>,
    pub sub_stat_2: Option<ArtifactStat>,
    pub sub_stat_3: Option<ArtifactStat>,
    pub sub_stat_4: Option<ArtifactStat>,
    pub pending_sub_stat: Option<ArtifactStat>,
    pub equip: Option<EquippedCharacter>,
}

impl Hash for ArtifactStat {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        let value = (self.value * 1000.0) as i32;
        value.hash(state);
    }
}

impl PartialEq for ArtifactStat {
    fn eq(&self, other: &Self) -> bool {
        if self.name != other.name {
            return false;
        }

        let value = (self.value * 1000.0) as i32;
        let other_value = (other.value * 1000.0) as i32;
        value == other_value
    }
}

impl Eq for ArtifactStat {}

impl ArtifactStatName {
    #[rustfmt::skip]
    pub fn from_zh_cn(name: &str, is_percentage: bool) -> Option<ArtifactStatName> {
        match name {
            "治疗加成" => Some(ArtifactStatName::HealingBonus),
            "暴击伤害" => Some(ArtifactStatName::CriticalDamage),
            "暴击率" => Some(ArtifactStatName::Critical),
            "攻击力" => if is_percentage { Some(ArtifactStatName::AtkPercentage) } else { Some(ArtifactStatName::Atk) },
            "元素精通" => Some(ArtifactStatName::ElementalMastery),
            "元素充能效率" => Some(ArtifactStatName::Recharge),
            "生命值" => if is_percentage { Some(ArtifactStatName::HpPercentage) } else { Some(ArtifactStatName::Hp) },
            "防御力" => if is_percentage { Some(ArtifactStatName::DefPercentage) } else { Some(ArtifactStatName::Def) },
            "雷元素伤害加成" => Some(ArtifactStatName::ElectroBonus),
            "火元素伤害加成" => Some(ArtifactStatName::PyroBonus),
            "水元素伤害加成" => Some(ArtifactStatName::HydroBonus),
            "冰元素伤害加成" => Some(ArtifactStatName::CryoBonus),
            "风元素伤害加成" => Some(ArtifactStatName::AnemoBonus),
            "岩元素伤害加成" => Some(ArtifactStatName::GeoBonus),
            "草元素伤害加成" => Some(ArtifactStatName::DendroBonus),
            "物理伤害加成" => Some(ArtifactStatName::PhysicalBonus),
            _ => None,
        }
    }
}

impl ArtifactStat {
    // Examples: "生命值+4,123", "暴击率+10%".
    pub fn from_zh_cn_raw(value: &str) -> Option<ArtifactStat> {
        let parts = value.split('+').collect::<Vec<_>>();
        if parts.len() != 2 {
            return None;
        }

        let is_percentage = parts[1].contains('%');
        let stat_name = ArtifactStatName::from_zh_cn(parts[0], is_percentage)?;
        let regex = Regex::new("[%,]").unwrap();
        let mut stat_value = match regex.replace_all(parts[1], "").parse::<f64>() {
            Ok(value) => value,
            Err(_) => {
                error!("stat `{value}` parse error");
                return None;
            },
        };
        if is_percentage {
            stat_value /= 100.0;
        }

        Some(ArtifactStat {
            name: stat_name,
            value: stat_value,
        })
    }
}

impl GenshinArtifact {
    pub fn from_scan_result(
        value: &GenshinArtifactScanResult,
        catalog: &ArtifactCatalog,
    ) -> Result<Self, ()> {
        let piece = catalog.find_piece(&[&value.name]).ok_or(())?;
        let main_stat = ArtifactStat::from_zh_cn_raw(
            &(value.main_stat_name.clone() + "+" + value.main_stat_value.as_str()),
        )
        .ok_or(())?;

        let mut sub_stats =
            std::array::from_fn(|index| ArtifactStat::from_zh_cn_raw(&value.sub_stat[index]));
        let pending_sub_stat = value
            .sub_stat_active
            .iter()
            .position(|active| !active)
            .and_then(|index| sub_stats[index].take());
        let [sub_stat_1, sub_stat_2, sub_stat_3, sub_stat_4] = sub_stats;

        Ok(GenshinArtifact {
            set_name: piece.set_name,
            slot: piece.slot,
            star: value.star,
            lock: value.lock,
            level: value.level,
            main_stat,
            sub_stat_1,
            sub_stat_2,
            sub_stat_3,
            sub_stat_4,
            pending_sub_stat,
            equip: catalog
                .find_character(&[&value.equip])
                .map(|matched| matched.character),
        })
    }
}

impl TryFrom<&GenshinArtifactScanResult> for GenshinArtifact {
    type Error = ();

    fn try_from(value: &GenshinArtifactScanResult) -> Result<Self, Self::Error> {
        let catalog = ArtifactCatalog::embedded().map_err(|_| ())?;
        Self::from_scan_result(value, &catalog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_artifact_with_data_driven_set_and_character() {
        let catalog = ArtifactCatalog::embedded().unwrap();
        let scan = GenshinArtifactScanResult {
            name: "风花的箴铭".to_string(),
            main_stat_name: "生命值".to_string(),
            main_stat_value: "4,780".to_string(),
            sub_stat: [
                "暴击率+3.9%".to_string(),
                "".to_string(),
                "".to_string(),
                "".to_string(),
            ],
            sub_stat_active: [true; 4],
            equip: "杜林已装备".to_string(),
            level: 20,
            star: 5,
            lock: true,
        };

        let artifact = GenshinArtifact::from_scan_result(&scan, &catalog).unwrap();
        assert_eq!(artifact.set_name.good_key(), "ADayCarvedFromRisingWinds");
        assert_eq!(artifact.slot, ArtifactSlot::Flower);
        assert_eq!(artifact.equip.unwrap().good_key(), "Durin");
    }

    #[test]
    fn preserves_but_does_not_activate_previewed_fourth_substat() {
        let catalog = ArtifactCatalog::embedded().unwrap();
        let scan = GenshinArtifactScanResult {
            name: "风花的箴铭".to_string(),
            main_stat_name: "生命值".to_string(),
            main_stat_value: "4,780".to_string(),
            sub_stat: [
                "暴击率+3.9%".to_string(),
                "攻击力+18".to_string(),
                "防御力+21".to_string(),
                "暴击伤害+7.8%".to_string(),
            ],
            sub_stat_active: [true, true, true, false],
            equip: String::new(),
            level: 0,
            star: 5,
            lock: false,
        };

        let artifact = GenshinArtifact::from_scan_result(&scan, &catalog).unwrap();
        assert!(artifact.sub_stat_4.is_none());
        assert_eq!(
            artifact.pending_sub_stat.unwrap().name,
            ArtifactStatName::CriticalDamage
        );
    }
}
