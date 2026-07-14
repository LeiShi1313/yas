use anyhow::{bail, Result};

use super::{LockChange, LockPlanEntry};

pub(crate) fn desired_lock_state(entry: LockPlanEntry, current: bool) -> Result<Option<bool>> {
    if entry.expected.is_some_and(|expected| expected != current) {
        bail!(
            "lock validation failed for index {}: expected {}, found {}",
            entry.target,
            entry.expected.unwrap(),
            current
        );
    }

    Ok(match entry.change {
        Some(LockChange::Flip) => Some(!current),
        Some(LockChange::Set(desired)) if desired != current => Some(desired),
        Some(LockChange::Set(_)) | None => None,
    })
}

pub(crate) fn parse_artifact_count(text: &str, maximum: usize) -> Result<usize> {
    if !text.contains("圣遗物") {
        bail!("artifact count label was not recognized: {text:?}");
    }
    let count = text
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| !part.is_empty())
        .ok_or_else(|| anyhow::anyhow!("artifact count was not recognized: {text:?}"))?
        .parse::<usize>()?;
    if count > maximum {
        bail!("artifact count {count} exceeds the supported maximum {maximum}");
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::{desired_lock_state, parse_artifact_count};
    use crate::scanner::{LockChange, LockPlanEntry};

    #[test]
    fn resolves_flip_and_absolute_changes() {
        let flip = LockPlanEntry {
            target: 2,
            expected: None,
            change: Some(LockChange::Flip),
        };
        assert_eq!(desired_lock_state(flip, false).unwrap(), Some(true));

        let already_locked = LockPlanEntry {
            target: 3,
            expected: None,
            change: Some(LockChange::Set(true)),
        };
        assert_eq!(desired_lock_state(already_locked, true).unwrap(), None);
    }

    #[test]
    fn rejects_a_validation_mismatch_before_changing_state() {
        let entry = LockPlanEntry {
            target: 5,
            expected: Some(false),
            change: Some(LockChange::Set(true)),
        };

        let error = desired_lock_state(entry, true).unwrap_err();
        assert!(error.to_string().contains("index 5"));
    }

    #[test]
    fn parses_a_strict_inventory_count() {
        assert_eq!(parse_artifact_count("圣遗物 123/2400", 2400).unwrap(), 123);
        assert!(parse_artifact_count("圣遗物", 2400).is_err());
        assert!(parse_artifact_count("圣遗物 2401/2400", 2400).is_err());
    }
}
