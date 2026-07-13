use std::cell::RefCell;
use std::ops::Coroutine;
use std::rc::Rc;
use std::time::SystemTime;

use anyhow::{anyhow, Context, Result};
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
    fast_pixels_per_event: f64,
    scroll_offset_y: f64,

    avg_switch_time: f64,
    scanned_count: usize,
    absolute_scroll_count: usize,
    page_first_scanned_count: usize,
    scan_start_row: usize,

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

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollbarGeometry {
    thumb_x: u32,
    thumb_top: u32,
    thumb_bottom: u32,
    track_top: u32,
    track_bottom: u32,
}

impl ScrollbarGeometry {
    fn thumb_half_height(self) -> f64 {
        (self.thumb_bottom - self.thumb_top) as f64 / 2.0
    }

    fn thumb_center(self) -> f64 {
        (self.thumb_top + self.thumb_bottom) as f64 / 2.0
    }

    fn top_center(self) -> f64 {
        self.track_top as f64 + self.thumb_half_height()
    }

    fn bottom_center(self) -> f64 {
        self.track_bottom as f64 - self.thumb_half_height()
    }

    fn travel(self) -> f64 {
        self.bottom_center() - self.top_center()
    }
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

fn luminance(pixel: &image::Rgb<u8>) -> f64 {
    pixel[0] as f64 * 0.2126 + pixel[1] as f64 * 0.7152 + pixel[2] as f64 * 0.0722
}

fn narrow_contrast(image: &RgbImage, y: u32, prefer_bright: bool) -> (f64, u32, u32) {
    let width = image.width();
    if width < 6 {
        return (f64::NEG_INFINITY, 0, 0);
    }

    let mut best = (f64::NEG_INFINITY, 0, 0);
    let max_span = 8.min(width - 2);
    for span_width in 3..=max_span {
        for start in 0..=width - span_width {
            let end = start + span_width;
            let inside = (start..end)
                .map(|x| luminance(image.get_pixel(x, y)))
                .sum::<f64>()
                / span_width as f64;
            let outside_count = width - span_width;
            let outside = (0..width)
                .filter(|x| *x < start || *x >= end)
                .map(|x| luminance(image.get_pixel(x, y)))
                .sum::<f64>()
                / outside_count as f64;
            let contrast = if prefer_bright {
                inside - outside
            } else {
                outside - inside
            };
            if contrast > best.0 {
                best = (contrast, start, end - 1);
            }
        }
    }
    best
}

fn extend_scrollbar_track(
    mask: &[bool],
    start: i32,
    direction: i32,
    max_missed: usize,
) -> Option<u32> {
    let mut index = start;
    let mut missed = 0;
    let mut last_match = None;
    while index >= 0 && (index as usize) < mask.len() {
        if mask[index as usize] {
            last_match = Some(index as u32);
            missed = 0;
        } else {
            missed += 1;
            if missed > max_missed {
                break;
            }
        }
        index += direction;
    }
    last_match
}

fn detect_scrollbar_geometry(image: &RgbImage) -> Option<ScrollbarGeometry> {
    if image.width() < 6 || image.height() < 40 {
        return None;
    }

    let bright_rows = (0..image.height())
        .map(|y| narrow_contrast(image, y, true))
        .collect::<Vec<_>>();
    let mut best_run = None;
    let mut run_start = None;
    let mut run_score = 0.0;
    let mut gap = 0;
    for (index, (score, _, _)) in bright_rows.iter().enumerate() {
        if *score >= 35.0 {
            if run_start.is_none() {
                run_start = Some(index);
                run_score = 0.0;
            }
            run_score += *score;
            gap = 0;
        } else if run_start.is_some() && gap < 1 {
            gap += 1;
        } else if let Some(start) = run_start.take() {
            let end = index.saturating_sub(gap + 1);
            if end + 1 - start >= 6
                && best_run
                    .map(|(_, _, score)| run_score > score)
                    .unwrap_or(true)
            {
                best_run = Some((start, end, run_score));
            }
            gap = 0;
        }
    }
    if let Some(start) = run_start {
        let end = bright_rows.len().saturating_sub(gap + 1);
        if end + 1 - start >= 6
            && best_run
                .map(|(_, _, score)| run_score > score)
                .unwrap_or(true)
        {
            best_run = Some((start, end, run_score));
        }
    }

    let (thumb_top, thumb_bottom, _) = best_run?;
    let center_row = (thumb_top + thumb_bottom) / 2;
    let (_, thumb_left, thumb_right) = bright_rows[center_row];
    let thumb_x = (thumb_left + thumb_right) / 2;

    let rail_mask = (0..image.height())
        .map(|y| {
            let inside = (thumb_left..=thumb_right)
                .map(|x| luminance(image.get_pixel(x, y)))
                .sum::<f64>()
                / (thumb_right - thumb_left + 1) as f64;
            let outside_count = image.width() - (thumb_right - thumb_left + 1);
            let outside = (0..image.width())
                .filter(|x| *x < thumb_left || *x > thumb_right)
                .map(|x| luminance(image.get_pixel(x, y)))
                .sum::<f64>()
                / outside_count as f64;
            outside - inside >= 3.0
        })
        .collect::<Vec<_>>();
    // The translucent rail can disappear against similarly colored inventory
    // cards for dozens of pixels, so follow it across bounded gaps.
    let max_rail_gap = (image.height() as usize / 10).clamp(6, 100);
    let track_top = extend_scrollbar_track(&rail_mask, thumb_top as i32 - 1, -1, max_rail_gap)
        .map(|value| value.min(thumb_top as u32))
        .unwrap_or(thumb_top as u32);
    let track_bottom = extend_scrollbar_track(&rail_mask, thumb_bottom as i32 + 1, 1, max_rail_gap)
        .map(|value| value.max(thumb_bottom as u32))
        .unwrap_or(thumb_bottom as u32);
    let geometry = ScrollbarGeometry {
        thumb_x,
        thumb_top: thumb_top as u32,
        thumb_bottom: thumb_bottom as u32,
        track_top,
        track_bottom,
    };
    if geometry.track_bottom - geometry.track_top < image.height() / 2 || geometry.travel() < 20.0 {
        return None;
    }
    Some(geometry)
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
            fast_pixels_per_event: 0.0,
            scroll_offset_y: 0.0,

            avg_switch_time: 0.0,
            // scanned_count: 0,
            game_info,
            scanned_count: 0,
            absolute_scroll_count: 0,
            page_first_scanned_count: 0,
            scan_start_row: 0,

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
            if !object.borrow_mut().calibrate_fast_scroll()? {
                info!("fast artifact scroll calibration unavailable; using conservative scrolling");
            }

            let row = object.borrow().row.min(total_row);

            'outer: while scanned_count < item_count {
                {
                    let mut controller = object.borrow_mut();
                    controller.page_first_scanned_count = scanned_count;
                    controller.scan_start_row = start_row;
                }
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

                        object.borrow_mut().select_item(row, col)?;

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

                if scanned_count >= item_count {
                    break;
                }

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
                        if !use_fast_scroll || object.borrow().scroll_offset_y.abs() < 0.01 {
                            object.borrow_mut().align_row()?;
                        }
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
                        if scanned_count < recovered_end {
                            let mut controller = object.borrow_mut();
                            controller.page_first_scanned_count = scanned_count;
                            controller.scan_start_row = (scanned_count - viewport_start) / columns;
                        }
                        while scanned_count < recovered_end {
                            object.borrow().ensure_game_foreground()?;
                            let offset = scanned_count - viewport_start;
                            object
                                .borrow_mut()
                                .select_item(offset / columns, offset % columns)?;
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
                            if scanned_count < new_row_end {
                                let mut controller = object.borrow_mut();
                                controller.page_first_scanned_count = scanned_count;
                                controller.scan_start_row =
                                    (scanned_count - viewport_start) / columns;
                            }
                            while scanned_count < new_row_end {
                                object.borrow().ensure_game_foreground()?;
                                let offset = scanned_count - viewport_start;
                                object
                                    .borrow_mut()
                                    .select_item(offset / columns, offset % columns)?;
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
    const GRID_STABLE_THRESHOLD: f64 = 0.05;
    const ROW_ALIGNMENT_THRESHOLD: f64 = 15.0;
    const FAST_SCROLL_MATCH_THRESHOLD: f64 = 40.0;
    const ROW_ALIGNMENT_VALLEY_RISE: f64 = 8.0;
    const FAST_SCROLL_CALIBRATION_ROWS: u32 = 3;
    const FAST_SCROLL_ROW_PITCH_TOLERANCE: u32 = 8;
    const FAST_SCROLL_EVENT_DELAY_MS: u32 = 2;
    const FAST_SCROLL_SETTLE_POLLS: usize = 8;
    const FAST_SCROLL_INPUT_SETTLE_MS: u32 = 150;
    const ABSOLUTE_SCROLL_SETTLE_MS: u32 = 100;
    const ABSOLUTE_SCROLL_RETRIES: usize = 5;

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

    fn scrollbar_rect(&self) -> Rect<i32> {
        let mut grid_top = self.window_info.scan_margin_pos.y;
        if self.is_artifact {
            grid_top += self.window_info.artifact_panel_offset.height;
        }
        let grid_height = (self.window_info.item_size.height
            + self.window_info.item_gap_size.height)
            * self.row as f64
            - self.window_info.item_gap_size.height;
        Rect {
            left: (self.window_info.panel_rect.left - 22.0) as i32,
            top: grid_top as i32,
            width: 18,
            height: grid_height as i32,
        }
    }

    fn capture_scrollbar(&self) -> Result<RgbImage> {
        let rect = self.scrollbar_rect();
        self.capturer
            .capture_relative_to(rect, self.game_info.window.origin())
    }

    fn reset_scroll_metrics(&mut self) {
        self.scrolled_rows = 0;
        self.avg_scroll_one_row = 0.0;
        self.avg_scroll_row_pitch = 0.0;
        self.fast_pixels_per_event = 0.0;
        self.scroll_offset_y = 0.0;
    }

    #[cfg(windows)]
    fn drag_scrollbar(&mut self, geometry: ScrollbarGeometry, target_center: f64) -> Result<()> {
        self.ensure_game_foreground()?;
        let rect = self.scrollbar_rect();
        let origin = self.game_info.window.origin();
        let x = origin.x + rect.left + geometry.thumb_x as i32;
        let current_y = origin.y + rect.top + geometry.thumb_center().round() as i32;
        let target_y = origin.y + rect.top + target_center.round() as i32;

        self.system_control
            .mouse_move_to(x, current_y)
            .with_context(|| {
                format!(
                    "failed to move to artifact scrollbar thumb at ({}, {})",
                    x, current_y
                )
            })?;
        utils::sleep(60);
        self.system_control.mouse_down()?;
        utils::sleep(60);
        let distance = target_y - current_y;
        let steps = ((distance.unsigned_abs() / 4) as usize).clamp(3, 12);
        let mut move_result = Ok(());
        for step in 1..=steps {
            let y = current_y + (distance as f64 * step as f64 / steps as f64).round() as i32;
            move_result = self.system_control.mouse_move_to(x, y);
            if move_result.is_err() {
                break;
            }
            utils::sleep(20);
        }
        utils::sleep(40);
        let release_result = self.system_control.mouse_up();
        move_result.with_context(|| {
            format!(
                "failed to drag artifact scrollbar thumb to ({}, {})",
                x, target_y
            )
        })?;
        release_result.context("failed to release artifact scrollbar thumb")?;
        utils::sleep(Self::ABSOLUTE_SCROLL_SETTLE_MS);
        Ok(())
    }

    #[cfg(windows)]
    fn try_scroll_to_first_direct(&mut self) -> Result<bool> {
        if !self.is_artifact {
            return Ok(false);
        }

        let scrollbar = self.capture_scrollbar()?;
        let mut geometry = match detect_scrollbar_geometry(&scrollbar) {
            Some(geometry) => geometry,
            None => {
                let _ = scrollbar
                    .save(std::env::temp_dir().join("yas-scrollbar-detect-scroll-to-first.png"));
                return Ok(false);
            },
        };
        let mut previous_center = None;
        for attempt in 0..Self::ABSOLUTE_SCROLL_RETRIES {
            // Move beyond the rail endpoint and let Genshin clamp the thumb to
            // its physical top; inferred track bounds are not precise enough
            // to establish the first row.
            self.drag_scrollbar(geometry, 0.0)?;
            self.absolute_scroll_count += 1;
            geometry = detect_scrollbar_geometry(&self.capture_scrollbar()?).ok_or_else(|| {
                anyhow!("lost scrollbar geometry after returning to the first artifact row")
            })?;
            if previous_center
                .map(|center: f64| (center - geometry.thumb_center()).abs() <= 1.0)
                .unwrap_or(false)
            {
                self.reset_scroll_metrics();
                info!("returned directly to the first artifact row");
                return Ok(true);
            }
            info!(
                "direct scroll-to-first attempt {} reached center {:.1}px",
                attempt + 1,
                geometry.thumb_center()
            );
            previous_center = Some(geometry.thumb_center());
        }
        Err(anyhow!(
            "unable to verify the first artifact row after direct scrollbar positioning"
        ))
    }

    #[cfg(not(windows))]
    fn try_scroll_to_first_direct(&mut self) -> Result<bool> {
        Ok(false)
    }

    pub fn ensure_game_foreground(&self) -> Result<()> {
        #[cfg(windows)]
        if !utils::is_foreground_window(self.game_info.window_handle) {
            if self.restore_focus {
                for _ in 0..3 {
                    utils::restore_foreground_window(self.game_info.window_handle);
                    utils::sleep(250);
                    if utils::is_foreground_window(self.game_info.window_handle) {
                        break;
                    }
                }
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

    pub fn absolute_scroll_count(&self) -> usize {
        self.absolute_scroll_count
    }

    pub fn scroll_offset_y(&self) -> f64 {
        self.scroll_offset_y
    }

    pub fn page_first_scanned_count(&self) -> usize {
        self.page_first_scanned_count
    }

    pub fn scan_start_row(&self) -> usize {
        self.scan_start_row
    }

    pub fn scroll_diagnostics(&self) -> String {
        format!(
            "slowEventsPerRow={:.3} measuredRowPitch={:.3} configuredRowPitch={:.3} fastPixelsPerEvent={:.3} offsetY={:.3}",
            self.avg_scroll_one_row,
            self.avg_scroll_row_pitch,
            self.window_info.item_size.height + self.window_info.item_gap_size.height,
            self.fast_pixels_per_event,
            self.scroll_offset_y
        )
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

    #[cfg(test)]
    fn sparse_shift_difference(before: &RgbImage, after: &RgbImage, shift: u32) -> f64 {
        if before.dimensions() != after.dimensions() || shift >= before.height() {
            return f64::INFINITY;
        }
        let comparison_height = (before.height() - shift).min(96);
        let mut total = 0u64;
        let mut samples = 0u64;
        for y in (0..comparison_height).step_by(4) {
            for x in (0..before.width()).step_by(16) {
                let old = before.get_pixel(x, y + shift);
                let new = after.get_pixel(x, y);
                for channel in 0..3 {
                    total += (old[channel] as i32 - new[channel] as i32).unsigned_abs() as u64;
                    samples += 1;
                }
            }
        }
        total as f64 / samples.max(1) as f64
    }

    #[cfg(test)]
    fn best_grid_shift(before: &RgbImage, after: &RgbImage) -> (u32, f64) {
        if before.dimensions() != after.dimensions() || before.height() < 2 {
            return (0, f64::INFINITY);
        }
        (1..before.height())
            .map(|shift| (shift, Self::sparse_shift_difference(before, after, shift)))
            .min_by(|left, right| left.1.total_cmp(&right.1))
            .unwrap_or((0, f64::INFINITY))
    }

    pub fn scroll_to_first(&mut self) -> Result<()> {
        self.ensure_game_foreground()?;
        self.move_to(0, 0)?;
        utils::sleep(100);

        if self.try_scroll_to_first_direct()? {
            return Ok(());
        }

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
                    self.reset_scroll_metrics();
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

    fn calibrate_fast_scroll(&mut self) -> Result<bool> {
        if !self.is_artifact {
            return Ok(false);
        }

        self.ensure_game_foreground()?;
        self.move_to(0, 0)?;
        let before = self.capture_grid()?;
        self.system_control.mouse_scroll(1, false)?;
        utils::sleep(Self::FAST_SCROLL_INPUT_SETTLE_MS);
        let after = self.capture_grid()?;

        let row_pitch = self.window_info.item_size.height + self.window_info.item_gap_size.height;
        let nominal_shift = (row_pitch / 8.0).round().max(1.0) as u32;
        let tolerance = nominal_shift.max(Self::FAST_SCROLL_ROW_PITCH_TOLERANCE);
        let (pixel_shift, difference) = Self::best_shift_difference(
            &before,
            &after,
            nominal_shift,
            tolerance,
            row_pitch.round().min(96.0) as u32,
        );

        self.system_control.mouse_scroll(-1, false)?;
        utils::sleep(Self::FAST_SCROLL_INPUT_SETTLE_MS);
        let restored = self.capture_grid()?;
        if difference > Self::ROW_ALIGNMENT_THRESHOLD
            || Self::mean_pixel_difference(&before, &restored) > Self::ROW_ALIGNMENT_THRESHOLD
        {
            self.scroll_to_first()?;
            return Ok(false);
        }

        self.scrolled_rows = Self::FAST_SCROLL_CALIBRATION_ROWS;
        self.avg_scroll_one_row = row_pitch / pixel_shift as f64;
        self.avg_scroll_row_pitch = row_pitch;
        self.fast_pixels_per_event = pixel_shift as f64;
        self.scroll_offset_y = 0.0;
        info!(
            "calibrated artifact scrolling at {:.2}px per wheel event",
            pixel_shift
        );
        Ok(true)
    }

    pub fn align_row(&mut self) -> Result<()> {
        for _ in 0..20 {
            self.ensure_game_foreground()?;
            let color = self.get_flag_color()?;

            if color_distance(&self.initial_color, &color) > 10 {
                self.system_control.mouse_scroll(1, false)?;
                utils::sleep(self.config.scroll_delay.try_into().unwrap());
            } else {
                self.scroll_offset_y = 0.0;
                return Ok(());
            }
        }
        Err(anyhow!(
            "unable to align artifact inventory row after scrollbar drag"
        ))
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
        let top = origin.y
            + margin.y
            + self.scroll_offset_y
            + (gap.height + size.height) * (row as f64)
            + size.height / 4.0;

        self.system_control
            .mouse_move_to(left as i32, top as i32)
            .with_context(|| {
                format!(
                    "failed to move to repository item row {}, column {} at ({}, {})",
                    row, col, left as i32, top as i32
                )
            })?;

        // SetCursorPos updates immediately at the OS level, but Genshin may
        // consume the click before its next input frame sees the new position.
        utils::sleep(20);

        Ok(())
    }

    fn select_item(&mut self, row: usize, col: usize) -> Result<()> {
        self.move_to(row, col)?;
        self.system_control.mouse_click()?;

        #[cfg(target_os = "macos")]
        utils::sleep(20);

        if self.wait_until_switched().is_err() {
            self.ensure_game_foreground()?;
            self.move_to(row, col)?;
            self.system_control.mouse_click()?;
            utils::sleep(self.config.scroll_delay.max(100) as u32);
        }
        Ok(())
    }

    fn settle_panel_pool(&mut self) -> Result<()> {
        let started = SystemTime::now();
        let mut previous = None;
        let mut stable_frames = 0;
        while started.elapsed()?.as_millis() < 500 {
            let image = self.capturer.capture_relative_to(
                self.window_info.pool_rect.to_rect_i32(),
                self.game_info.window.origin(),
            )?;
            let pool = calc_pool(image.as_raw()) as f64;
            if previous
                .map(|value: f64| (value - pool).abs() <= 0.000001)
                .unwrap_or(false)
            {
                stable_frames += 1;
                if stable_frames >= 2 {
                    self.pool = pool;
                    return Ok(());
                }
            } else {
                stable_frames = 0;
            }
            previous = Some(pool);
            utils::sleep(20);
        }
        if let Some(pool) = previous {
            self.pool = pool;
        }
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
        let grid_stable = best_image
            .as_ref()
            .map(|image| Self::mean_pixel_difference(&before, image) <= Self::GRID_STABLE_THRESHOLD)
            .unwrap_or(false);
        if let Some(best_image) = best_image {
            let _ = best_image.save(std::env::temp_dir().join("yas-scroll-best.png"));
        }
        if let Ok(scrollbar_after) = self.capture_scrollbar() {
            if grid_stable
                && Self::mean_pixel_difference(&scrollbar_before, &scrollbar_after)
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
        let is_full_page = scroll_rows >= self.row && remaining_items >= page_size;
        self.is_artifact
            && scroll_rows >= 3
            && self.scrolled_rows >= Self::FAST_SCROLL_CALIBRATION_ROWS
            && self.avg_scroll_one_row > 0.0
            && (is_full_page || remaining_items > page_size * 2)
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

        let before = match self.capture_grid() {
            Ok(image) => image,
            Err(_) => return ScrollResult::Failed,
        };
        let scrollbar_before = match self.capture_scrollbar() {
            Ok(image) => image,
            Err(_) => return ScrollResult::Failed,
        };
        // Window-info interpolation gives the physical row pitch. The shift
        // observed by slow scrolling can be an animation frame short of the
        // final position, so it is useful for matching but not as a burst target.
        let row_pitch = self.window_info.item_size.height + self.window_info.item_gap_size.height;
        let target_pixels = self.scroll_offset_y + row_pitch * count as f64;
        let tolerance = Self::row_pitch_tolerance(row_pitch.round() as u32) as f64;

        // A full-page scroll has no old row left on screen to correlate. Use
        // the calibrated carried-pixel plan from yas-lock, then verify that
        // both the grid and scrollbar actually moved. Item selection and the
        // consecutive-duplicate guard verify the new page while scanning it.
        if count as usize >= self.row && self.fast_pixels_per_event > 0.0 {
            let events = (target_pixels / self.fast_pixels_per_event)
                .round()
                .max(1.0) as i32;
            for _ in 0..events {
                if self.system_control.mouse_scroll(1, false).is_err() {
                    return ScrollResult::Failed;
                }
                utils::sleep(Self::FAST_SCROLL_EVENT_DELAY_MS);
            }
            utils::sleep(Self::FAST_SCROLL_INPUT_SETTLE_MS);

            let after = match self.capture_grid() {
                Ok(image) => image,
                Err(_) => return ScrollResult::Failed,
            };
            let scrollbar_after = match self.capture_scrollbar() {
                Ok(image) => image,
                Err(_) => return ScrollResult::Failed,
            };
            let grid_difference = Self::mean_pixel_difference(&before, &after);
            let scrollbar_difference =
                Self::mean_pixel_difference(&scrollbar_before, &scrollbar_after);
            if grid_difference <= Self::GRID_STABLE_THRESHOLD
                || scrollbar_difference <= Self::SCROLLBAR_STABLE_THRESHOLD
            {
                return ScrollResult::Failed;
            }

            let actual_shift = events as f64 * self.fast_pixels_per_event;
            self.scroll_offset_y = target_pixels - actual_shift;
            info!(
                "fast full-page scroll sent {} wheel events ({:.2}px carried)",
                events, self.scroll_offset_y
            );
            if self.settle_panel_pool().is_err() {
                return ScrollResult::Failed;
            }
            return ScrollResult::Success;
        }

        let mut sent_events = 0i32;
        let mut actual_shift = 0.0;
        let mut best_difference = f64::INFINITY;
        let mut differences = Vec::new();
        let mut after = before.clone();

        for attempt in 0..8 {
            if self.ensure_game_foreground().is_err() {
                return ScrollResult::FocusLost;
            }
            if utils::is_rmb_down() {
                return ScrollResult::Interrupt;
            }
            let remaining = target_pixels - actual_shift;
            if attempt > 0 && remaining.abs() <= tolerance {
                break;
            }
            if remaining < -tolerance {
                break;
            }
            let events = if sent_events == 0 && self.fast_pixels_per_event > 0.0 {
                (remaining / self.fast_pixels_per_event).round().max(1.0) as i32
            } else if sent_events == 0 {
                (self.avg_scroll_one_row * (count - 2).max(1) as f64)
                    .round()
                    .max(1.0) as i32
            } else {
                let observed_pixels_per_event = (actual_shift / sent_events as f64).max(1.0);
                ((remaining / observed_pixels_per_event) * 0.75)
                    .floor()
                    .max(1.0) as i32
            };
            let predicted_shift = if sent_events == 0 {
                let pixels_per_event = if self.fast_pixels_per_event > 0.0 {
                    self.fast_pixels_per_event
                } else {
                    row_pitch / self.avg_scroll_one_row.max(1.0)
                };
                events as f64 * pixels_per_event
            } else {
                let observed_pixels_per_event = actual_shift / sent_events as f64;
                actual_shift + events as f64 * observed_pixels_per_event
            };
            for _ in 0..events {
                if self.system_control.mouse_scroll(1, false).is_err() {
                    return ScrollResult::Failed;
                }
                utils::sleep(Self::FAST_SCROLL_EVENT_DELAY_MS);
            }
            sent_events += events;
            utils::sleep(Self::FAST_SCROLL_INPUT_SETTLE_MS);

            let mut measured = (0, f64::INFINITY);
            for _ in 0..Self::FAST_SCROLL_SETTLE_POLLS {
                after = match self.capture_grid() {
                    Ok(image) => image,
                    Err(_) => return ScrollResult::Failed,
                };
                let nominal_shift = predicted_shift
                    .round()
                    .clamp(1.0, (before.height() - 1) as f64)
                    as u32;
                let measurement_tolerance = if sent_events == events {
                    (row_pitch / 2.0).round() as u32
                } else {
                    (row_pitch / 5.0).round().max(16.0) as u32
                };
                measured = Self::best_shift_difference(
                    &before,
                    &after,
                    nominal_shift,
                    measurement_tolerance,
                    row_pitch.round().min(96.0) as u32,
                );
                if measured.1 <= Self::FAST_SCROLL_MATCH_THRESHOLD {
                    break;
                }
                utils::sleep(20);
            }
            actual_shift = measured.0 as f64;
            best_difference = best_difference.min(measured.1);
            differences.push(measured.1);
        }

        let grid_stable =
            Self::mean_pixel_difference(&before, &after) <= Self::GRID_STABLE_THRESHOLD;
        if let Ok(scrollbar_after) = self.capture_scrollbar() {
            if grid_stable
                && Self::mean_pixel_difference(&scrollbar_before, &scrollbar_after)
                    <= Self::SCROLLBAR_STABLE_THRESHOLD
            {
                return ScrollResult::EndReached;
            }
        }
        if best_difference > Self::FAST_SCROLL_MATCH_THRESHOLD
            || (target_pixels - actual_shift).abs() > tolerance
        {
            let _ = before.save(std::env::temp_dir().join("yas-fast-scroll-before.png"));
            let _ = after.save(std::env::temp_dir().join("yas-fast-scroll-after.png"));
            return ScrollResult::TimeLimitExceeded {
                best_difference,
                differences,
            };
        }
        self.fast_pixels_per_event = actual_shift / sent_events.max(1) as f64;
        self.scroll_offset_y = target_pixels - actual_shift;
        info!(
            "verified fast scroll moved {:.0}px with {} wheel events ({:.2}px carried)",
            actual_shift, sent_events, self.scroll_offset_y
        );
        utils::sleep(Self::FAST_SCROLL_INPUT_SETTLE_MS);
        if self.settle_panel_pool().is_err() {
            return ScrollResult::Failed;
        }
        ScrollResult::Success
    }

    pub fn scroll_rows_up(&mut self, count: i32) -> ScrollResult {
        if self.move_to(0, 0).is_err() {
            return ScrollResult::Failed;
        }
        utils::sleep(20);
        for _ in 0..count {
            let mut no_movement_retries = 0;
            loop {
                match self.scroll_one_row_up() {
                    ScrollResult::Success | ScrollResult::Skip => break,
                    ScrollResult::EndReached if no_movement_retries < 2 => {
                        no_movement_retries += 1;
                        utils::sleep(250);
                    },
                    result => return result,
                }
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
        detect_scrollbar_geometry, effective_switch_timeout, safe_clickable_artifact_rows,
        switch_frame_ready, GenshinRepositoryScanController, ScrollbarGeometry,
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

    fn scrollbar_image(thumb_top: u32, thumb_bottom: u32) -> RgbImage {
        let mut image = RgbImage::new(18, 210);
        for y in 0..image.height() {
            let background = 110 + (y / 5) as u8;
            for x in 0..image.width() {
                image.put_pixel(x, y, Rgb([background; 3]));
            }
        }
        for y in 30..=180 {
            for x in 2..=7 {
                let background = 110 + (y / 5) as u8;
                image.put_pixel(x, y, Rgb([background - 14; 3]));
            }
        }
        for y in thumb_top..=thumb_bottom {
            for x in 2..=7 {
                image.put_pixel(x, y, Rgb([220; 3]));
            }
        }
        image
    }

    #[test]
    fn scrollbar_geometry_tracks_thumb_at_top_middle_and_bottom() {
        for (thumb_top, thumb_bottom) in [(30, 44), (90, 104), (166, 180)] {
            let geometry = detect_scrollbar_geometry(&scrollbar_image(thumb_top, thumb_bottom));
            assert_eq!(
                geometry,
                Some(ScrollbarGeometry {
                    thumb_x: 4,
                    thumb_top,
                    thumb_bottom,
                    track_top: 30,
                    track_bottom: 180,
                })
            );
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
    fn sparse_grid_match_measures_non_row_aligned_motion() {
        let mut before = RgbImage::new(64, 300);
        for y in 0..before.height() {
            for x in 0..before.width() {
                before.put_pixel(x, y, Rgb([(y % 251) as u8, (x * 3) as u8, 17]));
            }
        }
        let mut after = RgbImage::new(64, 300);
        for y in 0..177 {
            for x in 0..after.width() {
                after.put_pixel(x, y, *before.get_pixel(x, y + 123));
            }
        }

        let (shift, difference) = GenshinRepositoryScanController::best_grid_shift(&before, &after);
        assert_eq!(shift, 123);
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
