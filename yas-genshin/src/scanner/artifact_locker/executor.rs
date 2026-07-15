use anyhow::{bail, Result};

use super::{LockChange, LockPlanEntry};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactInventoryCount {
    pub current: usize,
    pub capacity: usize,
}

pub(crate) fn resolve_artifact_item_count(
    inventory: ArtifactInventoryCount,
    configured: Option<usize>,
) -> Result<usize> {
    let Some(count) = configured else {
        return Ok(inventory.current);
    };
    if count > inventory.capacity {
        bail!(
            "configured artifact count {count} exceeds the OCR-detected inventory capacity {}",
            inventory.capacity
        );
    }
    if count > inventory.current {
        bail!(
            "configured artifact count {count} exceeds the OCR-detected current artifact count {}",
            inventory.current
        );
    }
    Ok(count)
}

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

pub(crate) fn parse_artifact_inventory_count(
    text: &str,
    capacity_hint: Option<&str>,
) -> Result<ArtifactInventoryCount> {
    if !text.contains("圣遗物") {
        bail!("artifact count label was not recognized: {text:?}");
    }

    let numbers = text
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(str::parse::<usize>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let (current, capacity) = match numbers.as_slice() {
        [current, capacity] => (*current, *capacity),
        [joined] => {
            let hinted_digits = capacity_hint
                .map(|hint| {
                    hint.chars()
                        .filter(char::is_ascii_digit)
                        .collect::<String>()
                })
                .filter(|digits| !digits.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "artifact capacity separator was not recognized and no capacity hint was available: {text:?}"
                    )
                })?;
            let joined_digits = joined.to_string();
            let split = hinted_digits
                .char_indices()
                .filter_map(|(start, character)| {
                    if character == '0' {
                        return None;
                    }
                    let capacity_digits = &hinted_digits[start..];
                    let current_digits = joined_digits
                        .strip_suffix(capacity_digits)
                        .filter(|digits| !digits.is_empty())?;
                    let current = current_digits.parse::<usize>().ok()?;
                    let capacity = capacity_digits.parse::<usize>().ok()?;
                    (current <= capacity).then_some((current, capacity))
                })
                .next()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "OCR capacity hint {capacity_hint:?} does not split artifact count {text:?}"
                    )
                })?;
            split
        },
        _ => bail!("artifact count and capacity were not recognized: {text:?}"),
    };
    if capacity == 0 {
        bail!("artifact capacity must be positive: {text:?}");
    }
    if current > capacity {
        bail!("artifact count {current} exceeds the inventory capacity {capacity}");
    }

    Ok(ArtifactInventoryCount { current, capacity })
}

#[cfg(test)]
mod tests {
    use super::{
        desired_lock_state, parse_artifact_inventory_count, resolve_artifact_item_count,
        ArtifactInventoryCount,
    };
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
    fn parses_inventory_count_and_dynamic_capacity() {
        assert_eq!(
            parse_artifact_inventory_count("圣遗物 2400/2400", None).unwrap(),
            ArtifactInventoryCount {
                current: 2400,
                capacity: 2400,
            }
        );
        assert_eq!(
            parse_artifact_inventory_count("圣遗物 123 / 2500", None).unwrap(),
            ArtifactInventoryCount {
                current: 123,
                capacity: 2500,
            }
        );
    }

    #[test]
    fn uses_a_separately_ocrd_capacity_when_the_slash_is_dropped() {
        assert_eq!(
            parse_artifact_inventory_count("圣遗物 24002400", Some("2400")).unwrap(),
            ArtifactInventoryCount {
                current: 2400,
                capacity: 2400,
            }
        );
        assert_eq!(
            parse_artifact_inventory_count("圣遗物 1232400", Some("/2400")).unwrap(),
            ArtifactInventoryCount {
                current: 123,
                capacity: 2400,
            }
        );
        assert_eq!(
            parse_artifact_inventory_count("圣遗物 24002400", Some("12400")).unwrap(),
            ArtifactInventoryCount {
                current: 2400,
                capacity: 2400,
            }
        );
    }

    #[test]
    fn rejects_ambiguous_or_impossible_inventory_counts() {
        assert!(parse_artifact_inventory_count("圣遗物 ???", None).is_err());
        assert!(parse_artifact_inventory_count("圣遗物 2400", None).is_err());
        assert!(parse_artifact_inventory_count("圣遗物 2401/2400", None).is_err());
        assert!(parse_artifact_inventory_count("圣遗物 1232400", Some("2500")).is_err());
        assert!(parse_artifact_inventory_count("武器 1000/2000", None).is_err());
    }

    #[test]
    fn configured_count_cannot_exceed_observed_inventory() {
        let inventory = ArtifactInventoryCount {
            current: 123,
            capacity: 2500,
        };
        assert_eq!(resolve_artifact_item_count(inventory, None).unwrap(), 123);
        assert_eq!(
            resolve_artifact_item_count(inventory, Some(100)).unwrap(),
            100
        );
        assert!(resolve_artifact_item_count(inventory, Some(124)).is_err());
        assert!(resolve_artifact_item_count(inventory, Some(2501)).is_err());
    }
}
