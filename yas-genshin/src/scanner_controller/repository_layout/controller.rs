use std::cell::RefCell;
use std::ops::Coroutine;
use std::rc::Rc;
use std::time::SystemTime;

use anyhow::{anyhow, Result};
use clap::{ArgMatches, FromArgMatches};
use image::RgbImage;
use log::{error, info};

use yas::capture::{Capturer, GenericCapturer};
use yas::game_info::GameInfo;
use yas::positioning::{Pos, Rect};
use yas::system_control::SystemControl;
use yas::utils;
use yas::window_info::{FromWindowInfoRepository, WindowInfoRepository};

use crate::scanner_controller::repository_layout::{
    GenshinRepositoryScanControllerWindowInfo, GenshinRepositoryScannerLogicConfig, ScrollResult,
};

pub struct GenshinRepositoryScanController {
    // to detect whether an item changes
    pool: f64,

    initial_color: image::Rgb<u8>,

    // for scrolls
    scrolled_rows: u32,
    avg_scroll_one_row: f64,
    avg_scroll_row_pitch: f64,
    scroll_remainder: f64,

    avg_switch_time: f64,
    scanned_count: usize,

    game_info: GameInfo,

    // row and column in one page
    row: usize,
    col: usize,

    config: GenshinRepositoryScannerLogicConfig,
    window_info: GenshinRepositoryScanControllerWindowInfo,
    system_control: SystemControl,
    capturer: Rc<dyn Capturer<RgbImage>>,

    // artifact panel have different layout
    is_artifact: bool,

    restore_focus: bool,
}

fn calc_pool(row: &[u8]) -> f32 {
    let len = row.len() / 3;
    let mut pool: f32 = 0.0;

    for i in 0..len {
        pool += row[i * 3] as f32;
    }
    pool
}

fn get_capturer() -> Result<Rc<dyn Capturer<RgbImage>>> {
    Ok(Rc::new(GenericCapturer::new()?))
}

fn color_distance(c1: &image::Rgb<u8>, c2: &image::Rgb<u8>) -> usize {
    let x = c1.0[0] as i32 - c2.0[0] as i32;
    let y = c1.0[1] as i32 - c2.0[1] as i32;
    let z = c1.0[2] as i32 - c2.0[2] as i32;
    (x * x + y * y + z * z) as usize
}

fn switch_frame_ready(consecutive_frames: u32, speed: u32) -> bool {
    consecutive_frames + speed >= 6
}

// Speed controls how many changed frames are required, not how long a slow switch may take.
fn effective_switch_timeout(max_wait_ms: i32) -> u128 {
    max_wait_ms.max(0) as u128
}

fn safe_clickable_artifact_rows(
    window_height: f64,
    configured_rows: i32,
    window_info: &GenshinRepositoryScanControllerWindowInfo,
) -> usize {
    let mut rows = configured_rows.max(1) as usize;
    let safe_bottom = window_height * 0.9;
    while rows > 1 {
        let row = rows - 1;
        let click_y = window_info.scan_margin_pos.y
            + window_info.artifact_panel_offset.height
            + (window_info.item_size.height + window_info.item_gap_size.height) * row as f64
            + window_info.item_size.height / 4.0;
        if click_y <= safe_bottom {
            break;
        }
        rows -= 1;
    }
    rows
}

// constructor
impl GenshinRepositoryScanController {
    pub fn new(
        window_info_repo: &WindowInfoRepository,
        config: GenshinRepositoryScannerLogicConfig,
        game_info: GameInfo,
        is_artifact: bool,
    ) -> Result<Self> {
        let window_info = GenshinRepositoryScanControllerWindowInfo::from_window_info_repository(
            game_info.window.to_rect_usize().size(),
            game_info.ui,
            game_info.platform,
            window_info_repo,
        )?;
        let configured_row = window_info.genshin_repository_item_row;
        let row = if is_artifact {
            safe_clickable_artifact_rows(
                game_info.window.height as f64,
                configured_row,
                &window_info,
            )
        } else {
            configured_row as usize
        };
        let col = window_info.genshin_repository_item_col;

        if row != configured_row as usize {
            info!(
                "using {} clickable artifact rows instead of {} visible rows",
                row, configured_row
            );
        }

        Ok(GenshinRepositoryScanController {
            system_control: SystemControl::new(),

            row,
            col: col as usize,

            window_info,
            config,

            pool: 0.0,

            initial_color: image::Rgb([0, 0, 0]),

            scrolled_rows: 0,
            avg_scroll_one_row: 0.0,
            avg_scroll_row_pitch: 0.0,
            scroll_remainder: 0.0,

            avg_switch_time: 0.0,
            // scanned_count: 0,
            game_info,
            scanned_count: 0,

            capturer: get_capturer()?,

            is_artifact,
            restore_focus: false,
        })
    }

    pub fn from_arg_matches(
        window_info_repo: &WindowInfoRepository,
        arg_matches: &ArgMatches,
        game_info: GameInfo,
        is_artifact: bool,
    ) -> Result<Self> {
        Self::new(
            window_info_repo,
            GenshinRepositoryScannerLogicConfig::from_arg_matches(arg_matches)?,
            game_info,
            is_artifact,
        )
    }
}

pub enum ReturnResult {
    Interrupted,
    Finished,
}

impl GenshinRepositoryScanController {
    pub fn get_generator(
        object: Rc<RefCell<GenshinRepositoryScanController>>,
        item_count: usize,
    ) -> impl Coroutine<Yield = (), Return = Result<ReturnResult>> {
        let generator = #[coroutine]
        move || {
            let mut scanned_row = 0;
            let mut scanned_count = 0;
            let mut start_row = 0;

            let total_row = (item_count + object.borrow().col - 1) / object.borrow().col;
            let last_row_col = if item_count % object.borrow().col == 0 {
                object.borrow().col
            } else {
                item_count % object.borrow().col
            };

            info!(
                "扫描任务共 {} 个物品，共计 {} 行，尾行 {} 个",
                item_count, total_row, last_row_col
            );

            object.borrow_mut().scroll_to_first()?;
            object.borrow().ensure_game_foreground()?;
            object.borrow_mut().move_to(0, 0)?;

            #[cfg(target_os = "macos")]
            utils::sleep(20);

            object.borrow_mut().system_control.mouse_click()?;
            utils::sleep(1000);

            object.borrow_mut().sample_initial_color()?;

            let row = object.borrow().row.min(total_row);

            'outer: while scanned_count < item_count {
                '_row: for row in start_row..row {
                    let row_item_count = if scanned_row == total_row - 1 {
                        last_row_col
                    } else {
                        object.borrow().col
                    };

                    '_col: for col in 0..row_item_count {
                        // 大于最大数量 或者 取消 或者 鼠标右键按下
                        if utils::is_rmb_down() {
                            return Ok(ReturnResult::Interrupted);
                        }
                        if scanned_count > item_count {
                            return Ok(ReturnResult::Finished);
                        }

                        object.borrow().ensure_game_foreground()?;

                        object.borrow_mut().move_to(row, col)?;
                        object.borrow_mut().system_control.mouse_click()?;

                        #[cfg(target_os = "macos")]
                        utils::sleep(20);

                        let _ = object.borrow_mut().wait_until_switched();

                        // have to make sure at this point no mut ref exists
                        yield;

                        scanned_count += 1;
                        object.borrow_mut().scanned_count = scanned_count;
                    } // end '_col

                    scanned_row += 1;

                    // todo this is dangerous, use uniform integer type instead
                    if scanned_row >= object.borrow().config.max_row as usize {
                        info!("到达最大行数，准备退出……");
                        break 'outer;
                    }
                } // end '_row

                let remain = item_count - scanned_count;
                let remain_row = (remain + object.borrow().col - 1) / object.borrow().col;
                let scroll_row = remain_row.min(object.borrow().row);
                start_row = object.borrow().row - scroll_row;

                let use_fast_scroll = {
                    let controller = object.borrow();
                    controller.can_fast_scroll(remain, scroll_row)
                };
                let scroll_result = if use_fast_scroll {
                    object.borrow_mut().scroll_rows_fast(scroll_row as i32)
                } else {
                    object.borrow_mut().scroll_rows(scroll_row as i32)
                };
                match scroll_result {
                    ScrollResult::Success | ScrollResult::Skip => {
                        object.borrow_mut().align_row();
                    },
                    ScrollResult::TimeLimitExceeded {
                        best_difference,
                        differences,
                    } => {
                        return Err(anyhow!(
                            "翻页对齐超时，最佳图像差异: {:.3}，序列: {:?}",
                            best_difference,
                            differences
                        ));
                    },
                    ScrollResult::Interrupt => {
                        return Ok(ReturnResult::Interrupted);
                    },
                    ScrollResult::EndReached => {
                        let remaining = item_count - scanned_count;
                        if remaining == 0 {
                            return Ok(ReturnResult::Finished);
                        }

                        let visible_rows = object.borrow().row;
                        let bottom_visible_rows = visible_rows.saturating_sub(1);
                        let columns = object.borrow().col;
                        let bottom_start_row = total_row.saturating_sub(bottom_visible_rows);
                        let bottom_start_index = bottom_start_row * columns;
                        let gap_items = bottom_start_index.saturating_sub(scanned_count);
                        if gap_items % columns != 0 {
                            return Err(anyhow!(
                                "cannot align final repository page: scanned={}, bottom_start={}",
                                scanned_count,
                                bottom_start_index
                            ));
                        }
                        let rows_up = gap_items / columns;
                        if rows_up > 0 {
                            let recovery_result =
                                object.borrow_mut().scroll_rows_up(rows_up as i32);
                            match recovery_result {
                                ScrollResult::Success | ScrollResult::Skip => (),
                                result => {
                                    return Err(anyhow!(
                                        "failed to recover the penultimate repository rows: {:?}",
                                        result
                                    ));
                                },
                            }
                        }

                        let mut viewport_start = bottom_start_index - rows_up * columns;
                        let recovered_end =
                            (viewport_start + bottom_visible_rows * columns).min(item_count);
                        while scanned_count < recovered_end {
                            object.borrow().ensure_game_foreground()?;
                            let offset = scanned_count - viewport_start;
                            object
                                .borrow_mut()
                                .move_to(offset / columns, offset % columns)?;
                            object.borrow_mut().system_control.mouse_click()?;
                            let _ = object.borrow_mut().wait_until_switched();
                            yield;
                            scanned_count += 1;
                            object.borrow_mut().scanned_count = scanned_count;
                        }

                        while viewport_start < bottom_start_index {
                            let next_viewport_start = viewport_start + columns;
                            let final_scroll_result = object.borrow_mut().scroll_one_row();
                            match final_scroll_result {
                                ScrollResult::Success | ScrollResult::Skip => (),
                                ScrollResult::EndReached
                                    if next_viewport_start == bottom_start_index => {},
                                result => {
                                    return Err(anyhow!(
                                        "failed to advance through final repository rows at {}: {:?}",
                                        viewport_start,
                                        result
                                    ));
                                },
                            }
                            viewport_start = next_viewport_start;

                            let new_row_start =
                                viewport_start + (bottom_visible_rows - 1) * columns;
                            let new_row_end = (new_row_start + columns).min(item_count);
                            while scanned_count < new_row_end {
                                object.borrow().ensure_game_foreground()?;
                                let offset = scanned_count - viewport_start;
                                object
                                    .borrow_mut()
                                    .move_to(offset / columns, offset % columns)?;
                                object.borrow_mut().system_control.mouse_click()?;
                                let _ = object.borrow_mut().wait_until_switched();
                                yield;
                                scanned_count += 1;
                                object.borrow_mut().scanned_count = scanned_count;
                            }
                        }

                        info!("Reached the final visible repository row");
                        return Ok(ReturnResult::Finished);
                    },
                    ScrollResult::FocusLost => {
                        return Err(anyhow!("Genshin lost foreground while scrolling"));
                    },
                    ScrollResult::Failed => {
                        return Err(anyhow!("failed to scroll the Genshin repository"));
                    },
                }

                utils::sleep(100);
            }

            Ok(ReturnResult::Finished)
        };

        generator
    }

    const SCROLL_TO_FIRST_BATCH_SIZE: usize = 20;
    const SCROLL_TO_FIRST_MAX_BATCHES: usize = 500;
    const SCROLL_TO_FIRST_EVENT_DELAY_MS: u32 = 10;
    const SCROLL_TO_FIRST_SETTLE_MS: u32 = 150;
    const SCROLLBAR_STABLE_THRESHOLD: f64 = 0.05;
    const ROW_ALIGNMENT_THRESHOLD: f64 = 15.0;
    const ROW_ALIGNMENT_VALLEY_RISE: f64 = 8.0;
    const FAST_SCROLL_CALIBRATION_ROWS: u32 = 3;
    const FAST_SCROLL_EXTRA_EVENTS: i32 = 8;
    const FAST_SCROLL_ROW_PITCH_TOLERANCE: u32 = 8;

    fn grid_rect(&self) -> Rect<i32> {
        let mut margin = self.window_info.scan_margin_pos;
        if self.is_artifact {
            margin = margin + self.window_info.artifact_panel_offset;
        }
        Rect {
            left: margin.x as i32,
            top: margin.y as i32,
            width: ((self.window_info.item_size.width + self.window_info.item_gap_size.width)
                * self.col as f64
                - self.window_info.item_gap_size.width) as i32,
            height: ((self.window_info.item_size.height + self.window_info.item_gap_size.height)
                * self.row as f64
                - self.window_info.item_gap_size.height) as i32,
        }
    }

    fn capture_grid(&self) -> Result<RgbImage> {
        self.capturer
            .capture_relative_to(self.grid_rect(), self.game_info.window.origin())
    }

    fn capture_scrollbar(&self) -> Result<RgbImage> {
        let mut grid_top = self.window_info.scan_margin_pos.y;
        if self.is_artifact {
            grid_top += self.window_info.artifact_panel_offset.height;
        }
        let grid_height = (self.window_info.item_size.height
            + self.window_info.item_gap_size.height)
            * self.row as f64
            - self.window_info.item_gap_size.height;
        let rect = Rect {
            left: (self.window_info.panel_rect.left - 22.0) as i32,
            top: grid_top as i32,
            width: 18,
            height: grid_height as i32,
        };
        self.capturer
            .capture_relative_to(rect, self.game_info.window.origin())
    }

    pub fn ensure_game_foreground(&self) -> Result<()> {
        #[cfg(windows)]
        if !utils::is_foreground_window(self.game_info.window_handle) {
            if self.restore_focus {
                utils::restore_foreground_window(self.game_info.window_handle);
                utils::sleep(250);
            }
        }
        #[cfg(windows)]
        if !utils::is_foreground_window(self.game_info.window_handle) {
            return Err(anyhow!(
                "Genshin is no longer the foreground window after {} captures",
                self.scanned_count
            ));
        }
        Ok(())
    }

    pub fn set_restore_focus(&mut self, restore_focus: bool) {
        self.restore_focus = restore_focus;
    }

    pub fn scanned_count(&self) -> usize {
        self.scanned_count
    }

    fn mean_pixel_difference(left: &RgbImage, right: &RgbImage) -> f64 {
        if left.dimensions() != right.dimensions() || left.as_raw().is_empty() {
            return f64::INFINITY;
        }
        let total = left
            .as_raw()
            .iter()
            .zip(right.as_raw())
            .map(|(left, right)| (*left as i32 - *right as i32).unsigned_abs() as u64)
            .sum::<u64>();
        total as f64 / left.as_raw().len() as f64
    }

    fn shifted_row_prefix_difference(
        before: &RgbImage,
        after: &RgbImage,
        row_shift: u32,
        prefix_height: u32,
    ) -> f64 {
        if before.dimensions() != after.dimensions()
            || row_shift >= before.height()
            || prefix_height == 0
        {
            return f64::INFINITY;
        }

        let comparison_height = prefix_height.min(before.height() - row_shift);
        let mut total = 0u64;
        let mut samples = 0u64;
        for y in (0..comparison_height).step_by(2) {
            for x in (0..before.width()).step_by(2) {
                let old = before.get_pixel(x, y + row_shift);
                let new = after.get_pixel(x, y);
                for channel in 0..3 {
                    total += (old[channel] as i32 - new[channel] as i32).unsigned_abs() as u64;
                    samples += 1;
                }
            }
        }
        total as f64 / samples as f64
    }

    fn row_pitch_tolerance(row_pitch: u32) -> u32 {
        Self::FAST_SCROLL_ROW_PITCH_TOLERANCE.max(row_pitch / 8)
    }

    fn best_shift_difference(
        before: &RgbImage,
        after: &RgbImage,
        nominal_shift: u32,
        tolerance: u32,
        prefix_height: u32,
    ) -> (u32, f64) {
        if before.dimensions() != after.dimensions() || before.height() < 2 {
            return (0, f64::INFINITY);
        }
        let min_shift = nominal_shift.saturating_sub(tolerance).max(1);
        let max_shift = (nominal_shift + tolerance).min(before.height() - 1);
        (min_shift..=max_shift)
            .map(|shift| {
                (
                    shift,
                    Self::shifted_row_prefix_difference(before, after, shift, prefix_height),
                )
            })
            .min_by(|left, right| left.1.total_cmp(&right.1))
            .unwrap_or((0, f64::INFINITY))
    }

    pub fn scroll_to_first(&mut self) -> Result<()> {
        self.move_to(0, 0)?;
        utils::sleep(100);

        let mut previous = self.capture_scrollbar()?;
        let mut stable_batches = 0;
        for batch in 0..Self::SCROLL_TO_FIRST_MAX_BATCHES {
            self.ensure_game_foreground()?;
            if utils::is_rmb_down() {
                return Err(anyhow!("scroll-to-first interrupted"));
            }
            for _ in 0..Self::SCROLL_TO_FIRST_BATCH_SIZE {
                self.system_control.mouse_scroll(-1, false)?;
                utils::sleep(Self::SCROLL_TO_FIRST_EVENT_DELAY_MS);
            }
            utils::sleep(Self::SCROLL_TO_FIRST_SETTLE_MS);

            let current = self.capture_scrollbar()?;
            let difference = Self::mean_pixel_difference(&previous, &current);
            if difference <= Self::SCROLLBAR_STABLE_THRESHOLD {
                stable_batches += 1;
                if stable_batches >= 3 {
                    self.scrolled_rows = 0;
                    self.avg_scroll_one_row = 0.0;
                    self.avg_scroll_row_pitch = 0.0;
                    self.scroll_remainder = 0.0;
                    info!("已回到背包第一件物品（{} 批滚轮）", batch + 1);
                    return Ok(());
                }
            } else {
                stable_batches = 0;
            }
            previous = current;
        }

        Err(anyhow!("unable to verify the first repository row"))
    }

    #[inline(always)]
    pub fn get_flag_color(&self) -> Result<image::Rgb<u8>> {
        let mut pos_f64 = Pos {
            x: self.window_info.flag_pos.x + self.game_info.window.left as f64,
            y: self.window_info.flag_pos.y + self.game_info.window.top as f64,
        };
        if self.is_artifact {
            pos_f64.x += self.window_info.artifact_panel_offset.width;
            pos_f64.y += self.window_info.artifact_panel_offset.height;
        }
        let pos_i32 = Pos {
            x: pos_f64.x as i32,
            y: pos_f64.y as i32,
        };
        self.capturer.capture_color(pos_i32)
    }

    #[inline(always)]
    pub fn sample_initial_color(&mut self) -> Result<()> {
        self.initial_color = self.get_flag_color()?;
        anyhow::Ok(())
    }

    pub fn align_row(&mut self) {
        for _ in 0..10 {
            let color = match self.get_flag_color() {
                Ok(color) => color,
                Err(_) => return,
            };

            if color_distance(&self.initial_color, &color) > 10 {
                self.mouse_scroll(1, false);
                utils::sleep(self.config.scroll_delay.try_into().unwrap());
            } else {
                break;
            }
        }
    }

    pub fn move_to(&mut self, row: usize, col: usize) -> Result<()> {
        let (row, col) = (row as u32, col as u32);
        let origin = self.game_info.window.to_rect_f64().origin();

        let gap = self.window_info.item_gap_size;
        let mut margin = self.window_info.scan_margin_pos;
        let size = self.window_info.item_size;
        if self.is_artifact {
            margin = margin + self.window_info.artifact_panel_offset;
        }

        let left = origin.x + margin.x + (gap.width + size.width) * (col as f64) + size.width / 2.0;
        let top =
            origin.y + margin.y + (gap.height + size.height) * (row as f64) + size.height / 4.0;

        self.system_control
            .mouse_move_to(left as i32, top as i32)?;

        #[cfg(target_os = "macos")]
        utils::sleep(20);

        Ok(())
    }

    fn scroll_one_row_direction(&mut self, direction: i32) -> ScrollResult {
        if self.ensure_game_foreground().is_err() {
            return ScrollResult::FocusLost;
        }
        let before = match self.capture_grid() {
            Ok(image) => image,
            Err(_) => return ScrollResult::Failed,
        };
        let scrollbar_before = match self.capture_scrollbar() {
            Ok(image) => image,
            Err(_) => return ScrollResult::Failed,
        };
        let row_pitch = if self.avg_scroll_row_pitch > 0.0 {
            self.avg_scroll_row_pitch.round() as u32
        } else {
            (self.window_info.item_size.height + self.window_info.item_gap_size.height) as u32
        };
        let max_scroll = 25;
        let mut best_difference = f64::INFINITY;
        let mut best_count = 0;
        let mut best_pixel_shift = row_pitch;
        let mut best_image = None;
        let mut differences = Vec::with_capacity(max_scroll);

        for count in 1..=max_scroll {
            if self.ensure_game_foreground().is_err() {
                return ScrollResult::FocusLost;
            }
            if utils::is_rmb_down() {
                return ScrollResult::Interrupt;
            }

            let _ = self.system_control.mouse_scroll(direction, false);
            utils::sleep(self.config.scroll_delay.try_into().unwrap());

            let current = match self.capture_grid() {
                Ok(image) => image,
                Err(_) => return ScrollResult::Failed,
            };
            let tolerance = Self::row_pitch_tolerance(row_pitch);
            let (pixel_shift, difference) = if direction > 0 {
                Self::best_shift_difference(&before, &current, row_pitch, tolerance, row_pitch / 2)
            } else {
                Self::best_shift_difference(&current, &before, row_pitch, tolerance, row_pitch / 2)
            };
            differences.push(difference);
            if difference < best_difference {
                best_difference = difference;
                best_count = count;
                best_pixel_shift = pixel_shift;
                best_image = Some(current.clone());
            }
            if difference <= Self::ROW_ALIGNMENT_THRESHOLD {
                self.update_avg_row(count as i32, pixel_shift);
                return ScrollResult::Success;
            }
            if best_count >= 3
                && count > best_count
                && difference >= best_difference + Self::ROW_ALIGNMENT_VALLEY_RISE
            {
                for _ in 0..count - best_count {
                    let _ = self.system_control.mouse_scroll(-direction, false);
                }
                utils::sleep(self.config.scroll_delay.max(20) as u32);
                self.update_avg_row(best_count as i32, best_pixel_shift);
                return ScrollResult::Success;
            }
        }

        let _ = before.save(std::env::temp_dir().join("yas-scroll-before.png"));
        if let Some(best_image) = best_image {
            let _ = best_image.save(std::env::temp_dir().join("yas-scroll-best.png"));
        }
        if let Ok(scrollbar_after) = self.capture_scrollbar() {
            if Self::mean_pixel_difference(&scrollbar_before, &scrollbar_after)
                <= Self::SCROLLBAR_STABLE_THRESHOLD
            {
                return ScrollResult::EndReached;
            }
        }
        ScrollResult::TimeLimitExceeded {
            best_difference,
            differences,
        }
    }

    pub fn scroll_one_row(&mut self) -> ScrollResult {
        self.scroll_one_row_direction(1)
    }

    fn scroll_one_row_up(&mut self) -> ScrollResult {
        self.scroll_one_row_direction(-1)
    }

    pub fn scroll_rows(&mut self, count: i32) -> ScrollResult {
        if self.move_to(0, 0).is_err() {
            return ScrollResult::Failed;
        }
        utils::sleep(20);
        for _ in 0..count {
            match self.scroll_one_row() {
                ScrollResult::Success | ScrollResult::Skip => continue,
                ScrollResult::Interrupt => return ScrollResult::Interrupt,
                v => {
                    error!("Scrolling failed: {:?}", v);
                    return v;
                },
            }
        }

        ScrollResult::Success
    }

    fn can_fast_scroll(&self, remaining_items: usize, scroll_rows: usize) -> bool {
        let page_size = self.row * self.col;
        self.is_artifact
            && scroll_rows >= 3
            && self.scrolled_rows >= Self::FAST_SCROLL_CALIBRATION_ROWS
            && self.avg_scroll_one_row > 0.0
            && remaining_items > page_size * 2
    }

    fn best_burst_alignment(
        before: &RgbImage,
        after: &RgbImage,
        row_pitch: u32,
        max_rows: i32,
    ) -> (i32, f64, Vec<f64>) {
        let differences = (1..=max_rows)
            .map(|rows| {
                let rows = rows as u32;
                let nominal_shift = row_pitch * rows;
                let tolerance = Self::row_pitch_tolerance(row_pitch) * rows;
                Self::best_shift_difference(before, after, nominal_shift, tolerance, row_pitch / 2)
                    .1
            })
            .collect::<Vec<_>>();
        let (index, difference) = differences
            .iter()
            .enumerate()
            .min_by(|left, right| left.1.total_cmp(right.1))
            .map(|(index, difference)| (index, *difference))
            .unwrap_or((0, f64::INFINITY));
        (index as i32 + 1, difference, differences)
    }

    fn scroll_rows_fast(&mut self, count: i32) -> ScrollResult {
        if count < 3
            || self.scrolled_rows < Self::FAST_SCROLL_CALIBRATION_ROWS
            || self.avg_scroll_one_row <= 0.0
        {
            return self.scroll_rows(count);
        }
        if self.move_to(0, 0).is_err() {
            return ScrollResult::Failed;
        }
        utils::sleep(20);
        if self.ensure_game_foreground().is_err() {
            return ScrollResult::FocusLost;
        }

        // Target two rows short because Genshin can coalesce a wheel burst into
        // more movement than the same events sent individually. We still allow
        // one extra row and keep an overlapping row to prove the exact shift.
        let burst_rows = count - 2;
        let max_aligned_rows = count - 1;
        let before = match self.capture_grid() {
            Ok(image) => image,
            Err(_) => return ScrollResult::Failed,
        };
        let scrollbar_before = match self.capture_scrollbar() {
            Ok(image) => image,
            Err(_) => return ScrollResult::Failed,
        };
        let row_pitch = if self.avg_scroll_row_pitch > 0.0 {
            self.avg_scroll_row_pitch.round() as u32
        } else {
            (self.window_info.item_size.height + self.window_info.item_gap_size.height) as u32
        };
        let desired_events = self.avg_scroll_one_row * burst_rows as f64 + self.scroll_remainder;
        let initial_events = (desired_events.floor() as i32).max(1);
        let max_events = initial_events + Self::FAST_SCROLL_EXTRA_EVENTS;
        let mut sent_events = 0;
        let mut last_differences = Vec::new();
        let mut best_difference = f64::INFINITY;

        while sent_events < max_events {
            if self.ensure_game_foreground().is_err() {
                return ScrollResult::FocusLost;
            }
            if utils::is_rmb_down() {
                return ScrollResult::Interrupt;
            }

            let events = if sent_events == 0 { initial_events } else { 1 };
            for _ in 0..events {
                if self.system_control.mouse_scroll(1, false).is_err() {
                    return ScrollResult::Failed;
                }
            }
            sent_events += events;
            utils::sleep(self.config.scroll_delay.max(20) as u32);

            let after = match self.capture_grid() {
                Ok(image) => image,
                Err(_) => return ScrollResult::Failed,
            };
            let (aligned_rows, difference, differences) =
                Self::best_burst_alignment(&before, &after, row_pitch, max_aligned_rows);
            best_difference = best_difference.min(difference);
            last_differences = differences;
            if difference <= Self::ROW_ALIGNMENT_THRESHOLD {
                self.scroll_remainder = desired_events - sent_events as f64;
                info!(
                    "fast scroll aligned {} rows with {} wheel events (difference {:.3})",
                    aligned_rows, sent_events, difference
                );
                return self.scroll_rows(count - aligned_rows);
            }
        }

        let _ = before.save(std::env::temp_dir().join("yas-fast-scroll-before.png"));
        if let Ok(after) = self.capture_grid() {
            let _ = after.save(std::env::temp_dir().join("yas-fast-scroll-after.png"));
        }
        if let Ok(scrollbar_after) = self.capture_scrollbar() {
            if Self::mean_pixel_difference(&scrollbar_before, &scrollbar_after)
                <= Self::SCROLLBAR_STABLE_THRESHOLD
            {
                return ScrollResult::EndReached;
            }
        }
        ScrollResult::TimeLimitExceeded {
            best_difference,
            differences: last_differences,
        }
    }

    pub fn scroll_rows_up(&mut self, count: i32) -> ScrollResult {
        if self.move_to(0, 0).is_err() {
            return ScrollResult::Failed;
        }
        utils::sleep(20);
        for _ in 0..count {
            match self.scroll_one_row_up() {
                ScrollResult::Success | ScrollResult::Skip => continue,
                result => return result,
            }
        }
        ScrollResult::Success
    }

    pub fn wait_until_switched(&mut self) -> Result<()> {
        if self.game_info.is_cloud {
            utils::sleep(self.config.cloud_wait_switch_item.try_into()?);
            return anyhow::Ok(());
        }

        let now = SystemTime::now();

        let mut consecutive_time = 0;
        let mut diff_flag = false;
        let timeout = effective_switch_timeout(self.config.max_wait_switch_item);
        while now.elapsed().unwrap().as_millis() < timeout {
            let im = self.capturer.capture_relative_to(
                self.window_info.pool_rect.to_rect_i32(),
                self.game_info.window.origin(),
            )?;

            let pool = calc_pool(im.as_raw()) as f64;

            if (pool - self.pool).abs() > 0.000001 {
                self.pool = pool;
                diff_flag = true;
                consecutive_time = 0;
            }
            if diff_flag {
                consecutive_time += 1;
                if switch_frame_ready(consecutive_time, self.config.switch_speed) {
                    self.avg_switch_time = (self.avg_switch_time * self.scanned_count as f64
                        + now.elapsed().unwrap().as_millis() as f64)
                        / (self.scanned_count as f64 + 1.0);
                    self.scanned_count += 1;
                    return anyhow::Ok(());
                }
            }
        }

        Err(anyhow!("Wait until switched failed"))
    }

    #[inline(always)]
    pub fn mouse_scroll(&mut self, length: i32, try_find: bool) {
        #[cfg(windows)]
        self.system_control.mouse_scroll(length, try_find).unwrap();

        #[cfg(target_os = "linux")]
        self.system_control.mouse_scroll(length, try_find).unwrap();

        #[cfg(target_os = "macos")]
        {
            match self.game_info.ui {
                crate::common::UI::Desktop => {
                    self.system_control.mouse_scroll(length);
                    utils::sleep(20);
                },
                crate::common::UI::Mobile => {
                    if try_find {
                        self.system_control.mac_scroll_fast(length);
                    } else {
                        self.system_control.mac_scroll_slow(length);
                    }
                },
            }
        }
    }

    #[inline(always)]
    fn update_avg_row(&mut self, count: i32, pixel_shift: u32) {
        let current = self.avg_scroll_one_row * self.scrolled_rows as f64 + count as f64;
        let current_pitch =
            self.avg_scroll_row_pitch * self.scrolled_rows as f64 + pixel_shift as f64;
        self.scrolled_rows += 1;
        self.avg_scroll_one_row = current / self.scrolled_rows as f64;
        self.avg_scroll_row_pitch = current_pitch / self.scrolled_rows as f64;

        info!(
            "avg scroll one row: {} events, {} px ({})",
            self.avg_scroll_one_row, self.avg_scroll_row_pitch, self.scrolled_rows
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        effective_switch_timeout, safe_clickable_artifact_rows, switch_frame_ready,
        GenshinRepositoryScanController,
    };
    use image::{Rgb, RgbImage};
    use yas::positioning::{Pos, Rect, Size};

    use crate::scanner_controller::repository_layout::GenshinRepositoryScanControllerWindowInfo;

    fn row_layout(
        rows: i32,
        margin_y: f64,
        artifact_offset_y: f64,
        item_height: f64,
        gap_height: f64,
    ) -> GenshinRepositoryScanControllerWindowInfo {
        GenshinRepositoryScanControllerWindowInfo {
            panel_rect: Rect::default(),
            flag_pos: Pos::default(),
            item_gap_size: Size {
                width: 0.0,
                height: gap_height,
            },
            item_size: Size {
                width: 0.0,
                height: item_height,
            },
            scan_margin_pos: Pos {
                x: 0.0,
                y: margin_y,
            },
            pool_rect: Rect::default(),
            artifact_panel_offset: Size {
                width: 0.0,
                height: artifact_offset_y,
            },
            genshin_repository_item_row: rows,
            genshin_repository_item_col: 8,
        }
    }

    #[test]
    fn scrollbar_difference_detects_stable_and_changed_frames() {
        let first = RgbImage::from_pixel(4, 4, Rgb([20, 30, 40]));
        let same = first.clone();
        let changed = RgbImage::from_pixel(4, 4, Rgb([30, 40, 50]));

        assert_eq!(
            GenshinRepositoryScanController::mean_pixel_difference(&first, &same),
            0.0
        );
        assert_eq!(
            GenshinRepositoryScanController::mean_pixel_difference(&first, &changed),
            10.0
        );
    }

    #[test]
    fn row_difference_matches_content_shifted_by_one_pitch() {
        let mut before = RgbImage::new(4, 8);
        for y in 0..before.height() {
            for x in 0..before.width() {
                before.put_pixel(x, y, Rgb([(y * 10) as u8, x as u8, 0]));
            }
        }
        let mut after = RgbImage::new(4, 8);
        for y in 0..6 {
            for x in 0..after.width() {
                after.put_pixel(x, y, *before.get_pixel(x, y + 2));
            }
        }

        assert_eq!(
            GenshinRepositoryScanController::shifted_row_prefix_difference(&before, &after, 2, 6),
            0.0
        );
    }

    #[test]
    fn burst_alignment_finds_the_exact_shift() {
        let mut before = RgbImage::new(4, 500);
        for y in 0..before.height() {
            for x in 0..before.width() {
                before.put_pixel(x, y, Rgb([(y % 251) as u8, x as u8, 0]));
            }
        }
        let mut after = RgbImage::new(4, 500);
        for y in 0..200 {
            for x in 0..after.width() {
                after.put_pixel(x, y, *before.get_pixel(x, y + 300));
            }
        }

        let (rows, difference, _) =
            GenshinRepositoryScanController::best_burst_alignment(&before, &after, 100, 4);
        assert_eq!(rows, 3);
        assert_eq!(difference, 0.0);
    }

    #[test]
    fn row_alignment_calibrates_away_from_the_profile_pitch() {
        let mut before = RgbImage::new(4, 300);
        for y in 0..before.height() {
            for x in 0..before.width() {
                before.put_pixel(x, y, Rgb([(y % 251) as u8, x as u8, 0]));
            }
        }
        let mut after = RgbImage::new(4, 300);
        for y in 0..206 {
            for x in 0..after.width() {
                after.put_pixel(x, y, *before.get_pixel(x, y + 94));
            }
        }

        let (shift, difference) =
            GenshinRepositoryScanController::best_shift_difference(&before, &after, 100, 12, 50);
        assert_eq!(shift, 94);
        assert_eq!(difference, 0.0);
    }

    #[test]
    fn bottom_layout_places_partial_last_row_in_fourth_clickable_row() {
        let item_count = 2364usize;
        let columns = 8usize;
        let configured_rows = 5usize;
        let bottom_visible_rows = configured_rows - 1;
        let total_rows = (item_count + columns - 1) / columns;
        let bottom_start = (total_rows - bottom_visible_rows) * columns;
        let final_row_offset = 2360 - bottom_start;

        assert_eq!(bottom_start, 2336);
        assert_eq!(final_row_offset / columns, 3);
        assert_eq!(final_row_offset % columns, 0);
    }

    #[test]
    fn switch_speed_five_accepts_the_first_changed_frame() {
        assert!(switch_frame_ready(1, 5));
        assert!(!switch_frame_ready(1, 4));
        assert!(switch_frame_ready(2, 4));
        assert_eq!(effective_switch_timeout(800), 800);
        assert_eq!(effective_switch_timeout(-1), 0);
    }

    #[test]
    fn clipped_bottom_rows_are_not_treated_as_clickable() {
        let sixteen_by_ten = row_layout(6, 91.0, 44.0, 113.0, 18.0);
        assert_eq!(safe_clickable_artifact_rows(900.0, 6, &sixteen_by_ten), 5);

        let four_by_three = row_layout(7, 81.0, 39.0, 101.0, 15.0);
        assert_eq!(safe_clickable_artifact_rows(960.0, 7, &four_by_three), 7);

        let sixteen_by_nine = row_layout(5, 101.0, 48.5, 126.0, 20.0);
        assert_eq!(safe_clickable_artifact_rows(900.0, 5, &sixteen_by_nine), 5);
    }
}
