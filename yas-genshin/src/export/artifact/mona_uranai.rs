use std::convert::From;

use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::artifact::{
    ArtifactSetName, ArtifactSlot, ArtifactStat, ArtifactStatName, GenshinArtifact,
};

type MonaArtifact = GenshinArtifact;

impl ArtifactStatName {
    pub fn to_mona(&self) -> String {
        let temp = match self {
            ArtifactStatName::HealingBonus => "cureEffect",
            ArtifactStatName::CriticalDamage => "criticalDamage",
            ArtifactStatName::Critical => "critical",
            ArtifactStatName::Atk => "attackStatic",
            ArtifactStatName::AtkPercentage => "attackPercentage",
            ArtifactStatName::ElementalMastery => "elementalMastery",
            ArtifactStatName::Recharge => "recharge",
            ArtifactStatName::HpPercentage => "lifePercentage",
            ArtifactStatName::Hp => "lifeStatic",
            ArtifactStatName::DefPercentage => "defendPercentage",
            ArtifactStatName::Def => "defendStatic",
            ArtifactStatName::ElectroBonus => "thunderBonus",
            ArtifactStatName::PyroBonus => "fireBonus",
            ArtifactStatName::HydroBonus => "waterBonus",
            ArtifactStatName::CryoBonus => "iceBonus",
            ArtifactStatName::AnemoBonus => "windBonus",
            ArtifactStatName::GeoBonus => "rockBonus",
            ArtifactStatName::PhysicalBonus => "physicalBonus",
            ArtifactStatName::DendroBonus => "dendroBonus",
        };
        String::from(temp)
    }
}

impl ArtifactSetName {
    pub fn to_mona(&self) -> String {
        match self.good_key() {
            "GladiatorsFinale" => "gladiatorFinale".to_string(),
            "Lavawalker" => "lavaWalker".to_string(),
            "WanderersTroupe" => "wandererTroupe".to_string(),
            "DefendersWill" => "defenderWill".to_string(),
            "TheExile" => "exile".to_string(),
            "ShimenawasReminiscence" => "shimenawaReminiscence".to_string(),
            "CrimsonWitchOfFlames" => "crimsonWitch".to_string(),
            "Thundersoother" => "thunderSmoother".to_string(),
            key if self.id() <= 15022 => {
                let mut chars = key.chars();
                match chars.next() {
                    Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            },
            key => key.to_string(),
        }
    }
}

impl ArtifactSlot {
    pub fn to_mona(&self) -> String {
        let temp = match self {
            ArtifactSlot::Flower => "flower",
            ArtifactSlot::Feather => "feather",
            ArtifactSlot::Sand => "sand",
            ArtifactSlot::Goblet => "cup",
            ArtifactSlot::Head => "head",
        };
        String::from(temp)
    }
}

impl Serialize for ArtifactStat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut root = serializer.serialize_map(Some(2))?;
        root.serialize_entry("name", &self.name.to_mona()).unwrap();
        root.serialize_entry("value", &self.value).unwrap();
        root.end()
    }
}

impl Serialize for MonaArtifact {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut root = serializer.serialize_map(Some(7))?;

        root.serialize_entry("setName", &self.set_name.to_mona())
            .unwrap();
        root.serialize_entry("position", &self.slot.to_mona())
            .unwrap();
        root.serialize_entry("mainTag", &self.main_stat).unwrap();

        let mut sub_stats: Vec<&ArtifactStat> = vec![];
        if let Some(ref s) = self.sub_stat_1 {
            sub_stats.push(s);
        }
        if let Some(ref s) = self.sub_stat_2 {
            sub_stats.push(s);
        }
        if let Some(ref s) = self.sub_stat_3 {
            sub_stats.push(s);
        }
        if let Some(ref s) = self.sub_stat_4 {
            sub_stats.push(s);
        }
        // let mut subs = serializer.serialize_seq(Some(sub_stats.len()))?;
        //
        // for i in sub_stats {
        //     subs.serialize_element(i);
        // }
        // subs.end();
        // subs.

        root.serialize_entry("normalTags", &sub_stats)?;
        root.serialize_entry("omit", &false)?;
        root.serialize_entry("level", &self.level)?;
        root.serialize_entry("star", &self.star)?;
        root.serialize_entry(
            "equip",
            &self.equip.as_ref().map(|character| character.name_zh_cn()),
        )?;
        // let random_id = thread_rng().gen::<u64>();
        // root.serialize_entry("id", &random_id);

        root.end()
    }
}

pub struct MonaFormat<'a> {
    version: String,
    flower: Vec<&'a MonaArtifact>,
    feather: Vec<&'a MonaArtifact>,
    cup: Vec<&'a MonaArtifact>,
    sand: Vec<&'a MonaArtifact>,
    head: Vec<&'a MonaArtifact>,
}

impl<'a> Serialize for MonaFormat<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut root = serializer.serialize_map(Some(6))?;
        root.serialize_entry("version", &self.version).unwrap();
        root.serialize_entry("flower", &self.flower).unwrap();
        root.serialize_entry("feather", &self.feather).unwrap();
        root.serialize_entry("sand", &self.sand).unwrap();
        root.serialize_entry("cup", &self.cup).unwrap();
        root.serialize_entry("head", &self.head).unwrap();
        root.end()
    }
}

impl<'a> MonaFormat<'a> {
    pub fn new(results: &[GenshinArtifact]) -> MonaFormat<'_> {
        let mut flower: Vec<&MonaArtifact> = Vec::new();
        let mut feather: Vec<&MonaArtifact> = Vec::new();
        let mut cup: Vec<&MonaArtifact> = Vec::new();
        let mut sand: Vec<&MonaArtifact> = Vec::new();
        let mut head: Vec<&MonaArtifact> = Vec::new();

        for art in results.iter() {
            match art.slot {
                ArtifactSlot::Flower => flower.push(art),
                ArtifactSlot::Feather => feather.push(art),
                ArtifactSlot::Sand => sand.push(art),
                ArtifactSlot::Goblet => cup.push(art),
                ArtifactSlot::Head => head.push(art),
            }
        }

        MonaFormat {
            flower,
            feather,
            cup,
            sand,
            head,
            version: String::from("1"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::artifact::ArtifactCatalog;

    #[test]
    fn preserves_legacy_mona_set_aliases() {
        let catalog = ArtifactCatalog::embedded().unwrap();
        assert_eq!(
            catalog
                .find_piece(&["魔女的炎之花"])
                .unwrap()
                .set_name
                .to_mona(),
            "crimsonWitch"
        );
        assert_eq!(
            catalog
                .find_piece(&["平雷之心"])
                .unwrap()
                .set_name
                .to_mona(),
            "thunderSmoother"
        );
    }
}
