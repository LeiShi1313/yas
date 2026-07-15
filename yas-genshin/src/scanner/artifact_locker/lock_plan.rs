use std::collections::BTreeMap;

use anyhow::{bail, Result};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockChange {
    Set(bool),
    Flip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LockPlanEntry {
    pub target: usize,
    pub expected: Option<bool>,
    pub change: Option<LockChange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockPlan {
    entries: Vec<LockPlanEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LockFormatV2 {
    version: u32,
    #[serde(default)]
    flip_indices: Vec<usize>,
    #[serde(default)]
    lock_indices: Vec<usize>,
    #[serde(default)]
    unlock_indices: Vec<usize>,
    #[serde(default)]
    validation: Vec<LockValidationRecord>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LockValidationRecord {
    index: usize,
    locked: bool,
}

impl LockPlan {
    pub fn from_json(json: &str) -> Result<Self> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        if value.is_array() {
            let indices = serde_json::from_value::<Vec<usize>>(value)?;
            return Self::normalize(
                indices
                    .into_iter()
                    .map(|target| (target, None, Some(LockChange::Flip))),
            );
        }

        let format = serde_json::from_value::<LockFormatV2>(value)?;
        if format.version != 2 {
            bail!("unsupported lock plan version {}", format.version);
        }

        let validation = format
            .validation
            .into_iter()
            .map(|record| (record.index, Some(record.locked), None));
        let flips = format
            .flip_indices
            .into_iter()
            .map(|target| (target, None, Some(LockChange::Flip)));
        let locks = format
            .lock_indices
            .into_iter()
            .map(|target| (target, None, Some(LockChange::Set(true))));
        let unlocks = format
            .unlock_indices
            .into_iter()
            .map(|target| (target, None, Some(LockChange::Set(false))));
        Self::normalize(validation.chain(flips).chain(locks).chain(unlocks))
    }

    pub fn entries(&self) -> &[LockPlanEntry] {
        &self.entries
    }

    fn normalize(
        records: impl IntoIterator<Item = (usize, Option<bool>, Option<LockChange>)>,
    ) -> Result<Self> {
        let mut entries = BTreeMap::<usize, LockPlanEntry>::new();
        for (target, expected, change) in records {
            let entry = entries.entry(target).or_insert(LockPlanEntry {
                target,
                expected: None,
                change: None,
            });
            if expected.is_some() && entry.expected.replace(expected.unwrap()).is_some() {
                bail!("conflicting validations for index {target}");
            }
            if change.is_some() && entry.change.replace(change.unwrap()).is_some() {
                bail!("conflicting changes for index {target}");
            }
        }
        Ok(Self {
            entries: entries.into_values().collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{LockChange, LockPlan, LockPlanEntry};

    #[test]
    fn parses_and_sorts_v1_flip_indices() {
        let plan = LockPlan::from_json("[8, 2, 5]").unwrap();

        assert_eq!(
            plan.entries(),
            &[
                LockPlanEntry {
                    target: 2,
                    expected: None,
                    change: Some(LockChange::Flip),
                },
                LockPlanEntry {
                    target: 5,
                    expected: None,
                    change: Some(LockChange::Flip),
                },
                LockPlanEntry {
                    target: 8,
                    expected: None,
                    change: Some(LockChange::Flip),
                },
            ]
        );
    }

    #[test]
    fn parses_v2_absolute_changes_and_validation() {
        let plan = LockPlan::from_json(
            r#"{
                "version": 2,
                "flip_indices": [9],
                "lock_indices": [5],
                "unlock_indices": [2],
                "validation": [
                    {"index": 5, "locked": false},
                    {"index": 7, "locked": true}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(
            plan.entries(),
            &[
                LockPlanEntry {
                    target: 2,
                    expected: None,
                    change: Some(LockChange::Set(false)),
                },
                LockPlanEntry {
                    target: 5,
                    expected: Some(false),
                    change: Some(LockChange::Set(true)),
                },
                LockPlanEntry {
                    target: 7,
                    expected: Some(true),
                    change: None,
                },
                LockPlanEntry {
                    target: 9,
                    expected: None,
                    change: Some(LockChange::Flip),
                },
            ]
        );
    }

    #[test]
    fn rejects_conflicting_changes_for_one_index() {
        let error = LockPlan::from_json(
            r#"{
                "version": 2,
                "flip_indices": [],
                "lock_indices": [3],
                "unlock_indices": [3],
                "validation": []
            }"#,
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("conflicting changes for index 3"));
    }

    #[test]
    fn rejects_duplicate_v1_flip_indices() {
        let error = LockPlan::from_json("[4, 4]").unwrap_err();

        assert!(error
            .to_string()
            .contains("conflicting changes for index 4"));
    }

    #[test]
    fn rejects_unknown_v2_fields_instead_of_silently_doing_nothing() {
        let error = LockPlan::from_json(
            r#"{
                "version": 2,
                "validations": [{"index": 0, "locked": true}]
            }"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `validations`"));
    }
}
