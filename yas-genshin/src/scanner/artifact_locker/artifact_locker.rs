use std::cell::RefCell;
use std::ops::{Coroutine, CoroutineState};
use std::pin::Pin;
use std::rc::Rc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::FromArgMatches;
use image::RgbImage;
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
    desired_lock_state, parse_artifact_count, GenshinArtifactLockerConfig, LockPlan, LockPlanEntry,
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
    const MAX_COUNT: usize = 2400;

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

        Ok(Self {
            config,
            configured_item_count,
            window_info,
            controller: Rc::new(RefCell::new(
                GenshinRepositoryScanController::from_arg_matches(
                    window_info_repo,
                    arg_matches,
                    game_info.clone(),
                    true,
                )?,
            )),
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
        if let Some(count) = self.configured_item_count {
            if count > Self::MAX_COUNT {
                bail!(
                    "configured artifact count {count} exceeds the supported maximum {}",
                    Self::MAX_COUNT
                );
            }
            return Ok(count);
        }

        let image = self.capturer.capture_relative_to(
            self.window_info.item_count_rect.to_rect_i32(),
            self.game_info.window.origin(),
        )?;
        let text = self.image_to_text.image_to_text(&image, false)?;
        info!("artifact count label: {text}");
        parse_artifact_count(&text, Self::MAX_COUNT).with_context(|| {
            "could not safely determine the artifact count; pass --number with the exact count"
        })
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

    fn click_lock_button(&mut self) -> Result<()> {
        self.controller.borrow().ensure_game_foreground()?;
        let x = self.game_info.window.left + self.window_info.detail_lock_pos.x as i32;
        let y = self.game_info.window.top + self.window_info.detail_lock_pos.y as i32;
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
            self.click_lock_button()?;
            self.wait_for_lock_state(position, desired)?;
            report.changed += 1;
        } else if entry.change.is_some() {
            report.already_desired += 1;
        }
        report.processed += 1;
        Ok(())
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

        let targets = plan.entries().iter().map(|entry| entry.target).collect();
        let mut generator = GenshinRepositoryScanController::get_target_generator(
            self.controller.clone(),
            item_count,
            targets,
        );
        for entry in plan.entries().iter().copied() {
            let position = match Pin::new(&mut generator).resume(()) {
                CoroutineState::Yielded(position) if position.index == entry.target => position,
                CoroutineState::Yielded(position) => bail!(
                    "repository navigation yielded index {}, expected {}",
                    position.index,
                    entry.target
                ),
                CoroutineState::Complete(Ok(ReturnResult::Interrupted)) => {
                    report.interrupted = true;
                    break;
                },
                CoroutineState::Complete(Ok(ReturnResult::Finished)) => {
                    bail!("repository navigation ended before index {}", entry.target)
                },
                CoroutineState::Complete(Err(error)) => return Err(error),
            };
            self.apply_entry(entry, position, &mut report)
                .with_context(|| {
                    format!(
                        "artifact lock plan stopped at index {} after {} confirmed changes",
                        entry.target, report.changed
                    )
                })?;
            if report.interrupted {
                break;
            }
        }
        Ok(report)
    }
}
