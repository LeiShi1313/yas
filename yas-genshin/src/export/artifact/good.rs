use serde::ser::{SerializeMap, Serializer};
use serde::Serialize;

use crate::artifact::{ArtifactSlot, ArtifactStat, ArtifactStatName, GenshinArtifact};

struct GOODArtifact<'a> {
    artifact: &'a GenshinArtifact,
}

impl Serialize for GOODArtifact<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let artifact = self.artifact;
        let substats = [
            &artifact.sub_stat_1,
            &artifact.sub_stat_2,
            &artifact.sub_stat_3,
            &artifact.sub_stat_4,
        ]
        .into_iter()
        .flatten()
        .map(GOODStat::new)
        .collect::<Vec<_>>();

        let mut root = serializer.serialize_map(Some(8))?;
        root.serialize_entry("setKey", artifact.set_name.good_key())?;
        root.serialize_entry("slotKey", artifact.slot.to_good())?;
        root.serialize_entry("level", &artifact.level)?;
        root.serialize_entry("rarity", &artifact.star)?;
        root.serialize_entry("mainStatKey", artifact.main_stat.name.to_good())?;
        root.serialize_entry(
            "location",
            artifact
                .equip
                .as_ref()
                .map(|character| character.good_key())
                .unwrap_or(""),
        )?;
        root.serialize_entry("lock", &artifact.lock)?;
        root.serialize_entry("substats", &substats)?;
        root.end()
    }
}

#[derive(Serialize)]
struct GOODStat<'a> {
    key: &'a str,
    value: f64,
}

impl<'a> GOODStat<'a> {
    fn new(stat: &'a ArtifactStat) -> GOODStat<'a> {
        GOODStat {
            key: stat.name.to_good(),
            value: match stat.name {
                ArtifactStatName::Atk
                | ArtifactStatName::ElementalMastery
                | ArtifactStatName::Hp
                | ArtifactStatName::Def => stat.value,
                _ => stat.value * 100.0,
            },
        }
    }
}

impl ArtifactStatName {
    pub fn to_good(&self) -> &'static str {
        match self {
            ArtifactStatName::HealingBonus => "heal_",
            ArtifactStatName::CriticalDamage => "critDMG_",
            ArtifactStatName::Critical => "critRate_",
            ArtifactStatName::Atk => "atk",
            ArtifactStatName::AtkPercentage => "atk_",
            ArtifactStatName::ElementalMastery => "eleMas",
            ArtifactStatName::Recharge => "enerRech_",
            ArtifactStatName::HpPercentage => "hp_",
            ArtifactStatName::Hp => "hp",
            ArtifactStatName::DefPercentage => "def_",
            ArtifactStatName::Def => "def",
            ArtifactStatName::ElectroBonus => "electro_dmg_",
            ArtifactStatName::PyroBonus => "pyro_dmg_",
            ArtifactStatName::HydroBonus => "hydro_dmg_",
            ArtifactStatName::CryoBonus => "cryo_dmg_",
            ArtifactStatName::AnemoBonus => "anemo_dmg_",
            ArtifactStatName::GeoBonus => "geo_dmg_",
            ArtifactStatName::PhysicalBonus => "physical_dmg_",
            ArtifactStatName::DendroBonus => "dendro_dmg_",
        }
    }
}

impl ArtifactSlot {
    pub fn to_good(&self) -> &'static str {
        match self {
            ArtifactSlot::Flower => "flower",
            ArtifactSlot::Feather => "plume",
            ArtifactSlot::Sand => "sands",
            ArtifactSlot::Goblet => "goblet",
            ArtifactSlot::Head => "circlet",
        }
    }
}

#[derive(Serialize)]
pub struct GOODFormat<'a> {
    format: &'a str,
    version: u32,
    source: &'a str,
    artifacts: Vec<GOODArtifact<'a>>,
}

impl<'a> GOODFormat<'a> {
    pub fn new(results: &'a [GenshinArtifact]) -> GOODFormat<'a> {
        let artifacts = results
            .iter()
            .map(|artifact| GOODArtifact { artifact })
            .collect();
        GOODFormat {
            format: "GOOD",
            version: 1,
            source: "yas",
            artifacts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{ArtifactCatalog, GenshinArtifact};
    use crate::scanner::GenshinArtifactScanResult;

    #[test]
    fn exports_new_sets_and_characters_without_static_tables() {
        let catalog = ArtifactCatalog::embedded().unwrap();
        let scan = GenshinArtifactScanResult {
            name: "风花的箴铭".to_string(),
            main_stat_name: "生命值".to_string(),
            main_stat_value: "4,780".to_string(),
            sub_stat: [String::new(), String::new(), String::new(), String::new()],
            sub_stat_active: [true; 4],
            equip: "杜林已装备".to_string(),
            level: 20,
            star: 5,
            lock: false,
        };
        let artifact = GenshinArtifact::from_scan_result(&scan, &catalog).unwrap();
        let output = serde_json::to_value(GOODFormat::new(&[artifact])).unwrap();
        assert_eq!(
            output["artifacts"][0]["setKey"],
            "ADayCarvedFromRisingWinds"
        );
        assert_eq!(output["artifacts"][0]["location"], "Durin");
    }
}
