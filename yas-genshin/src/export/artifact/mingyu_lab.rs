use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::artifact::{
    ArtifactSetName, ArtifactSlot, ArtifactStat, ArtifactStatName, GenshinArtifact,
};

struct MingyuLabArtifact<'a> {
    artifact: &'a GenshinArtifact,
}

impl<'a> Serialize for MingyuLabArtifact<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let extract_stat_name = |maybe_stat: &Option<ArtifactStat>| match maybe_stat {
            None => "flatATK",
            Some(stat) => stat.name.to_mingyu_lab(),
        };

        let extract_stat_value = |maybe_stat: &Option<ArtifactStat>| match maybe_stat {
            None => 0.0,
            Some(stat) => match stat.name {
                ArtifactStatName::Atk
                | ArtifactStatName::ElementalMastery
                | ArtifactStatName::Hp
                | ArtifactStatName::Def => stat.value,
                _ => stat.value * 100.0,
            },
        };

        let artifact = &self.artifact;
        let mut root = serializer.serialize_map(Some(13))?;
        root.serialize_entry("asKey", &artifact.set_name.to_mingyu_lab())?;
        root.serialize_entry("rarity", &artifact.star)?;
        root.serialize_entry("slot", artifact.slot.to_mingyu_lab())?;
        root.serialize_entry("level", &artifact.level)?;
        root.serialize_entry("mainStat", artifact.main_stat.name.to_mingyu_lab())?;
        root.serialize_entry("subStat1Type", &extract_stat_name(&artifact.sub_stat_1))?;
        root.serialize_entry("subStat1Value", &extract_stat_value(&artifact.sub_stat_1))?;
        root.serialize_entry("subStat2Type", &extract_stat_name(&artifact.sub_stat_2))?;
        root.serialize_entry("subStat2Value", &extract_stat_value(&artifact.sub_stat_2))?;
        root.serialize_entry("subStat3Type", &extract_stat_name(&artifact.sub_stat_3))?;
        root.serialize_entry("subStat3Value", &extract_stat_value(&artifact.sub_stat_3))?;
        root.serialize_entry("subStat4Type", &extract_stat_name(&artifact.sub_stat_4))?;
        root.serialize_entry("subStat4Value", &extract_stat_value(&artifact.sub_stat_4))?;
        root.end()
    }
}

impl ArtifactStatName {
    pub fn to_mingyu_lab(&self) -> &'static str {
        match self {
            ArtifactStatName::HealingBonus => "healing",
            ArtifactStatName::CriticalDamage => "critDamage",
            ArtifactStatName::Critical => "critRate",
            ArtifactStatName::Atk => "flatATK",
            ArtifactStatName::AtkPercentage => "percentATK",
            ArtifactStatName::ElementalMastery => "elementalMastery",
            ArtifactStatName::Recharge => "energyRecharge",
            ArtifactStatName::HpPercentage => "percentHP",
            ArtifactStatName::Hp => "flatHP",
            ArtifactStatName::DefPercentage => "percentDEF",
            ArtifactStatName::Def => "flatDEF",
            ArtifactStatName::ElectroBonus => "electroDamage",
            ArtifactStatName::PyroBonus => "pyroDamage",
            ArtifactStatName::HydroBonus => "hydroDamage",
            ArtifactStatName::CryoBonus => "cryoDamage",
            ArtifactStatName::AnemoBonus => "anemoDamage",
            ArtifactStatName::GeoBonus => "geoDamage",
            ArtifactStatName::PhysicalBonus => "physicalDamage",
            ArtifactStatName::DendroBonus => "dendroDamage",
        }
    }
}

impl ArtifactSlot {
    pub fn to_mingyu_lab(&self) -> &'static str {
        match self {
            ArtifactSlot::Flower => "flower",
            ArtifactSlot::Feather => "plume",
            ArtifactSlot::Sand => "eon",
            ArtifactSlot::Goblet => "goblet",
            ArtifactSlot::Head => "circlet",
        }
    }
}

impl ArtifactSetName {
    pub fn to_mingyu_lab(&self) -> String {
        match self.good_key() {
            "BlizzardStrayer" => "blizzard_walker".to_string(),
            "EmblemOfSeveredFate" => "seal_of_insulation".to_string(),
            "ShimenawasReminiscence" => "reminiscence_of_shime".to_string(),
            "OceanHuedClam" => "divine_chorus".to_string(),
            "MarechausseeHunter" => "hunter".to_string(),
            "FlowerOfParadiseLost" => "flower_of_paradise_list".to_string(),
            "PrayersForDestiny" => "prayers_of_destiny".to_string(),
            "PrayersForIllumination" => "prayers_of_illumination".to_string(),
            "PrayersForWisdom" => "prayers_of_wisdom".to_string(),
            "PrayersToSpringtime" => "prayers_of_springtime".to_string(),
            key => camel_to_snake(key),
        }
    }
}

fn camel_to_snake(value: &str) -> String {
    let mut result = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index > 0 && ch.is_ascii_uppercase() {
            result.push('_');
        }
        result.extend(ch.to_lowercase());
    }
    result
}

pub struct MingyuLabFormat<'a> {
    artifacts: Vec<MingyuLabArtifact<'a>>,
}

impl<'a> MingyuLabFormat<'a> {
    pub fn new(results: &'a [GenshinArtifact]) -> MingyuLabFormat<'a> {
        let artifacts: Vec<MingyuLabArtifact<'a>> = results
            .iter()
            .filter(|artifact| {
                !matches!(
                    artifact.set_name.good_key(),
                    "Adventurer" | "LuckyDog" | "TravelingDoctor"
                )
            })
            .map(|artifact| MingyuLabArtifact { artifact })
            .collect();
        MingyuLabFormat { artifacts }
    }
}

impl Serialize for MingyuLabFormat<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.artifacts.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use crate::artifact::ArtifactCatalog;

    #[test]
    fn preserves_legacy_mingyu_set_aliases() {
        let catalog = ArtifactCatalog::embedded().unwrap();
        assert_eq!(
            catalog
                .find_piece(&["祭水礼冠"])
                .unwrap()
                .set_name
                .to_mingyu_lab(),
            "prayers_of_destiny"
        );
        assert_eq!(
            catalog
                .find_piece(&["月女的华彩"])
                .unwrap()
                .set_name
                .to_mingyu_lab(),
            "flower_of_paradise_list"
        );
    }
}
