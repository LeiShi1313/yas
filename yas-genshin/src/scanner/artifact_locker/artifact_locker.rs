use std::cell::RefCell;
use std::ops::{Coroutine, CoroutineState};
use std::pin::Pin;
use std::rc::Rc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::FromArgMatches;
use image::{GenericImageView, RgbImage};
use log::info;

use yas::capture::{Capturer, GenericCapturer};
use yas::game_info::GameInfo;
use yas::ocr::{yas_ocr_model, ImageToText};
use yas::positioning::Rect;
use yas::system_control::SystemControl;
use yas::utils;
use yas::window_info::{FromWindowInfoRepository, WindowInfoRepository};

use crate::scanner::artifact_lock_state::has_lock_marker;
use crate::scanner::artifact_scanner::ArtifactScannerWindowInfo;
use crate::scanner::artifact_scanner::GenshinArtifactScannerConfig;
use crate::scanner_controller::repository_layout::{
    GenshinRepositoryScanController, RepositoryItemPosition, ReturnResult,
};

use super::{
    desired_lock_state, parse_artifact_inventory_count, resolve_artifact_item_count,
    GenshinArtifactLockerConfig, LockPlan, LockPlanEntry,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GenshinArtifactLockReport {
    pub processed: usize,
    pub changed: usize,
    pub already_desired: usize,
    pub validated: usize,
    pub interrupted: bool,
}

pub struct GenshinArtifactLocker {
    config: GenshinArtifactLockerConfig,
    configured_item_count: Option<usize>,
    window_info: ArtifactScannerWindowInfo,
    game_info: GameInfo,
    image_to_text: Box<dyn ImageToText<RgbImage> + Send>,
    controller: Rc<RefCell<GenshinRepositoryScanController>>,
    capturer: Rc<dyn Capturer<RgbImage>>,
    system_control: SystemControl,
}

impl GenshinArtifactLocker {
    pub fn from_arg_matches(
        window_info_repo: &WindowInfoRepository,
        arg_matches: &clap::ArgMatches,
        game_info: GameInfo,
    ) -> Result<Self> {
        let config = GenshinArtifactLockerConfig::from_arg_matches(arg_matches)?;
        let scanner_config = GenshinArtifactScannerConfig::from_arg_matches(arg_matches)?;
        let configured_item_count = (scanner_config.number > 0)
            .then_some(scanner_config.number)
            .map(usize::try_from)
            .transpose()?;
        let window_info = ArtifactScannerWindowInfo::from_window_info_repository(
            game_info.window.to_rect_usize().size(),
            game_info.ui,
            game_info.platform,
            window_info_repo,
        )?;
        let image_to_text: Box<dyn ImageToText<RgbImage> + Send> = Box::new(yas_ocr_model!(
            "../artifact_scanner/models/model_training.onnx",
            "../artifact_scanner/models/index_2_word.json"
        )?);

        let mut controller = GenshinRepositoryScanController::from_arg_matches(
            window_info_repo,
            arg_matches,
            game_info.clone(),
            true,
        )?;
        controller.set_restore_focus(true);

        Ok(Self {
            config,
            configured_item_count,
            window_info,
            controller: Rc::new(RefCell::new(controller)),
            game_info,
            image_to_text,
            capturer: Rc::new(GenericCapturer::new()?),
            system_control: SystemControl::new(),
        })
    }

    pub fn set_restore_focus(&mut self, restore_focus: bool) {
        self.controller
            .borrow_mut()
            .set_restore_focus(restore_focus);
    }

    fn get_item_count(&self) -> Result<usize> {
        self.controller.borrow().ensure_game_foreground()?;
        let image = self.capturer.capture_relative_to(
            self.window_info.item_count_rect.to_rect_i32(),
            self.game_info.window.origin(),
        )?;
        let text = self.image_to_text.image_to_text(&image, false)?;
        let capacity_crop_left = image.width() * 2 / 3;
        let capacity_image = image
            .view(
                capacity_crop_left,
                0,
                image.width() - capacity_crop_left,
                image.height(),
            )
            .to_image();
        let capacity_text = self.image_to_text.image_to_text(&capacity_image, false)?;
        info!("artifact count label: {text}; capacity segment: {capacity_text}");
        let inventory =
            parse_artifact_inventory_count(&text, Some(&capacity_text)).with_context(|| {
                "could not safely determine the artifact inventory count and capacity"
            })?;
        info!(
            "artifact inventory count: {}, capacity: {}",
            inventory.current, inventory.capacity
        );

        resolve_artifact_item_count(inventory, self.configured_item_count)
    }

    fn capture_lock_state(&self, position: RepositoryItemPosition) -> Result<bool> {
        let gap = self.window_info.item_gap_size;
        let size = self.window_info.item_size;
        let margin = self.window_info.scan_margin_pos + self.window_info.artifact_panel_offset;
        let scroll_offset_y = self.controller.borrow().scroll_offset_y();
        let center_x = self.game_info.window.left as f64
            + margin.x
            + (gap.width + size.width) * position.col as f64
            + self.window_info.lock_pos.x;
        let center_y = self.game_info.window.top as f64
            + margin.y
            + scroll_offset_y
            + (gap.height + size.height) * position.row as f64
            + self.window_info.lock_pos.y;
        let image = self.capturer.capture_rect(Rect {
            left: center_x as i32 - 1,
            top: center_y as i32 - 10,
            width: 2,
            height: 20,
        })?;
        Ok(has_lock_marker(&image, 1, 10))
    }

    fn click_lock_button(&mut self, vertical_offset: i32) -> Result<()> {
        self.controller.borrow().ensure_game_foreground()?;
        let x = self.game_info.window.left + self.window_info.detail_lock_pos.x as i32;
        let y =
            self.game_info.window.top + self.window_info.detail_lock_pos.y as i32 + vertical_offset;
        self.system_control
            .mouse_move_to(x, y)
            .with_context(|| format!("failed to move to the artifact lock button at ({x}, {y})"))?;
        utils::sleep(20);
        self.system_control
            .mouse_click()
            .context("failed to click the artifact lock button")?;
        utils::sleep(self.config.lock_stop);
        Ok(())
    }

    fn change_lock_state(&mut self, position: RepositoryItemPosition, desired: bool) -> Result<()> {
        // Genshin 6.7 inserts a definition banner above the controls for some
        // artifacts, moving both the lock and star buttons down by 37px at
        // 1920x1080. The base and shifted rows are both verified against the
        // lock marker in the repository grid before execution continues.
        let definition_banner_shift =
            (self.game_info.window.height as f64 * 37.0 / 1080.0).round() as i32;
        let mut last_error = None;
        for vertical_offset in [0, definition_banner_shift] {
            self.click_lock_button(vertical_offset)?;
            match self.wait_for_lock_state(position, desired) {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("artifact lock state did not change")))
            .with_context(|| {
                format!(
                    "lock button did not set artifact index {} to {} at either supported detail-row position",
                    position.index,
                    if desired { "locked" } else { "unlocked" }
                )
            })
    }

    fn wait_for_lock_state(&self, position: RepositoryItemPosition, desired: bool) -> Result<()> {
        let started = Instant::now();
        while started.elapsed().as_millis() < self.config.max_wait_lock as u128 {
            if self.capture_lock_state(position)? == desired {
                return Ok(());
            }
            utils::sleep(20);
        }
        bail!(
            "timed out after {}ms waiting for artifact index {} to become {}",
            self.config.max_wait_lock,
            position.index,
            if desired { "locked" } else { "unlocked" }
        )
    }

    fn apply_entry(
        &mut self,
        entry: LockPlanEntry,
        position: RepositoryItemPosition,
        report: &mut GenshinArtifactLockReport,
        changed_states: &mut Vec<(usize, bool)>,
    ) -> Result<()> {
        let current = self.capture_lock_state(position)?;
        if entry.expected.is_some() {
            report.validated += 1;
        }
        let desired = desired_lock_state(entry, current)?;
        if let Some(desired) = desired {
            if utils::is_rmb_down() {
                report.interrupted = true;
                return Ok(());
            }
            // Record the original state before clicking. If the click succeeds
            // but its verification fails, rollback can still restore this target.
            changed_states.push((entry.target, current));
            self.change_lock_state(position, desired)?;
            report.changed += 1;
        } else if entry.change.is_some() {
            report.already_desired += 1;
        }
        report.processed += 1;
        Ok(())
    }

    fn preflight_validations(&mut self, plan: &LockPlan, item_count: usize) -> Result<()> {
        let validation_entries = plan
            .entries()
            .iter()
            .copied()
            .filter(|entry| entry.expected.is_some())
            .collect::<Vec<_>>();
        if validation_entries.is_empty() {
            return Ok(());
        }

        let targets = validation_entries
            .iter()
            .map(|entry| entry.target)
            .collect();
        let mut generator = GenshinRepositoryScanController::get_target_generator(
            self.controller.clone(),
            item_count,
            targets,
        );
        for entry in validation_entries {
            let position = match Pin::new(&mut generator).resume(()) {
                CoroutineState::Yielded(position) if position.index == entry.target => position,
                CoroutineState::Yielded(position) => bail!(
                    "validation navigation yielded index {}, expected {}",
                    position.index,
                    entry.target
                ),
                CoroutineState::Complete(Ok(ReturnResult::Interrupted)) => {
                    bail!("artifact lock validation preflight was interrupted")
                },
                CoroutineState::Complete(Ok(ReturnResult::Finished)) => {
                    bail!("validation navigation ended before index {}", entry.target)
                },
                CoroutineState::Complete(Err(error)) => return Err(error),
            };
            let current = self.capture_lock_state(position)?;
            desired_lock_state(
                LockPlanEntry {
                    change: None,
                    ..entry
                },
                current,
            )
            .with_context(|| {
                format!(
                    "artifact lock validation failed at index {} (detected state: {})",
                    entry.target,
                    if current { "locked" } else { "unlocked" }
                )
            })?;
        }
        Ok(())
    }

    fn rollback_changes(
        &mut self,
        item_count: usize,
        changed_states: &[(usize, bool)],
    ) -> Result<()> {
        if changed_states.is_empty() {
            return Ok(());
        }

        let targets = changed_states.iter().map(|(target, _)| *target).collect();
        let mut generator = GenshinRepositoryScanController::get_target_generator(
            self.controller.clone(),
            item_count,
            targets,
        );
        for &(target, original) in changed_states {
            let position = match Pin::new(&mut generator).resume(()) {
                CoroutineState::Yielded(position) if position.index == target => position,
                CoroutineState::Yielded(position) => bail!(
                    "rollback navigation yielded index {}, expected {}",
                    position.index,
                    target
                ),
                CoroutineState::Complete(Ok(ReturnResult::Interrupted)) => {
                    bail!("artifact lock rollback was interrupted")
                },
                CoroutineState::Complete(Ok(ReturnResult::Finished)) => {
                    bail!("rollback navigation ended before index {target}")
                },
                CoroutineState::Complete(Err(error)) => return Err(error),
            };
            let current = self.capture_lock_state(position)?;
            if current != original {
                self.change_lock_state(position, original)?;
            }
        }
        Ok(())
    }

    fn error_after_rollback(
        &mut self,
        error: anyhow::Error,
        item_count: usize,
        changed_states: &[(usize, bool)],
    ) -> anyhow::Error {
        let changed_count = changed_states.len();
        match self.rollback_changes(item_count, changed_states) {
            Ok(()) => error.context(format!(
                "artifact lock plan failed with {changed_count} touched targets; all touched targets were restored to their original states"
            )),
            Err(rollback_error) => error.context(format!(
                "artifact lock plan failed with {changed_count} touched targets; rollback also failed: {rollback_error:#}"
            )),
        }
    }

    pub fn execute(&mut self, plan: &LockPlan) -> Result<GenshinArtifactLockReport> {
        let mut report = GenshinArtifactLockReport::default();
        if plan.entries().is_empty() {
            return Ok(report);
        }

        let item_count = self.get_item_count()?;
        let last_target = plan.entries().last().unwrap().target;
        if last_target >= item_count {
            bail!("artifact lock target {last_target} is outside the inventory count {item_count}");
        }

        self.preflight_validations(plan, item_count)
            .context("artifact lock validation preflight failed before any changes")?;

        let targets = plan.entries().iter().map(|entry| entry.target).collect();
        let mut generator = GenshinRepositoryScanController::get_target_generator(
            self.controller.clone(),
            item_count,
            targets,
        );
        let mut changed_states = Vec::new();
        for entry in plan.entries().iter().copied() {
            let position = match Pin::new(&mut generator).resume(()) {
                CoroutineState::Yielded(position) if position.index == entry.target => position,
                CoroutineState::Yielded(position) => {
                    let error = anyhow::anyhow!(
                        "repository navigation yielded index {}, expected {}",
                        position.index,
                        entry.target
                    );
                    return Err(self.error_after_rollback(error, item_count, &changed_states));
                },
                CoroutineState::Complete(Ok(ReturnResult::Interrupted)) => {
                    report.interrupted = true;
                    break;
                },
                CoroutineState::Complete(Ok(ReturnResult::Finished)) => {
                    let error = anyhow::anyhow!(
                        "repository navigation ended before index {}",
                        entry.target
                    );
                    return Err(self.error_after_rollback(error, item_count, &changed_states));
                },
                CoroutineState::Complete(Err(error)) => {
                    return Err(self.error_after_rollback(error, item_count, &changed_states));
                },
            };
            if let Err(error) = self
                .apply_entry(entry, position, &mut report, &mut changed_states)
                .with_context(|| {
                    format!(
                        "artifact lock plan stopped at index {} after {} confirmed changes",
                        entry.target, report.changed
                    )
                })
            {
                return Err(self.error_after_rollback(error, item_count, &changed_states));
            }
            if report.interrupted {
                break;
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod window_info_tests {
    use yas::game_info::{Platform, UI};
    use yas::positioning::Size;
    use yas::window_info::{load_window_info_repo, FromWindowInfoRepository};

    use crate::scanner::artifact_scanner::ArtifactScannerWindowInfo;
    use crate::scanner_controller::repository_layout::GenshinRepositoryScanControllerWindowInfo;

    #[test]
    fn every_bundled_windows_profile_contains_artifact_locking_coordinates() {
        let repository = load_window_info_repo!(
            "../../../window_info/windows1600x900.json",
            "../../../window_info/windows1280x960.json",
            "../../../window_info/windows1440x900.json",
            "../../../window_info/windows2100x900.json",
            "../../../window_info/windows3440x1440.json",
        );

        for (width, height) in [
            (1600, 900),
            (1280, 960),
            (1440, 900),
            (2100, 900),
            (3440, 1440),
        ] {
            let size = Size { width, height };
            ArtifactScannerWindowInfo::from_window_info_repository(
                size,
                UI::Desktop,
                Platform::Windows,
                &repository,
            )
            .unwrap();
            GenshinRepositoryScanControllerWindowInfo::from_window_info_repository(
                size,
                UI::Desktop,
                Platform::Windows,
                &repository,
            )
            .unwrap();
        }
    }

    #[test]
    fn bundled_lock_button_coordinates_match_the_verified_layouts() {
        let repository = load_window_info_repo!(
            "../../../window_info/windows1600x900.json",
            "../../../window_info/windows1280x960.json",
            "../../../window_info/windows1440x900.json",
            "../../../window_info/windows2100x900.json",
            "../../../window_info/windows3440x1440.json",
        );

        for (width, height, expected_x, expected_y) in [
            (1280, 960, 1131.0, 294.0),
            (1440, 900, 1273.0, 331.0),
            (1600, 900, 1415.0, 369.0),
            (1920, 1080, 1698.0, 442.8),
            (2100, 900, 1861.0, 383.0),
            (3440, 1440, 3058.0, 613.0),
        ] {
            let info = ArtifactScannerWindowInfo::from_window_info_repository(
                Size { width, height },
                UI::Desktop,
                Platform::Windows,
                &repository,
            )
            .unwrap();
            assert!(
                (info.detail_lock_pos.x - expected_x).abs() < 0.01,
                "unexpected lock x at {width}x{height}: {}",
                info.detail_lock_pos.x
            );
            assert!(
                (info.detail_lock_pos.y - expected_y).abs() < 0.01,
                "unexpected lock y at {width}x{height}: {}",
                info.detail_lock_pos.y
            );
        }
    }
}
