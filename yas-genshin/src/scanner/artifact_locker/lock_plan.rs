use anyhow::{bail, Result};

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

impl LockPlan {
    pub fn from_json(json: &str) -> Result<Self> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        let Some(indices) = value.as_array() else {
            bail!("lock plan object parsing is not implemented")
        };
        let mut entries = indices
            .iter()
            .map(|value| {
                let target = value.as_u64().ok_or_else(|| {
                    anyhow::anyhow!("v1 lock indices must be non-negative integers")
                })?;
                Ok(LockPlanEntry {
                    target: usize::try_from(target)?,
                    expected: None,
                    change: Some(LockChange::Flip),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.target);
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[LockPlanEntry] {
        &self.entries
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
}
