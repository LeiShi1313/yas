use std::collections::HashSet;
use std::sync::{mpsc::Receiver, Arc};
use std::thread::JoinHandle;

use anyhow::Result;
use edit_distance::edit_distance;
use image::{GenericImageView, RgbImage};
use log::{error, info, warn};

use yas::ocr::{yas_ocr_model, ImageToText, PPOCRChV4RecInfer};
use yas::positioning::{Pos, Rect};

use crate::artifact::{ArtifactCatalog, ArtifactSlot, ArtifactStat, ArtifactStatName};
use crate::scanner::artifact_lock_state::has_lock_marker;
use crate::scanner::artifact_scanner::artifact_scanner_window_info::ArtifactScannerWindowInfo;
use crate::scanner::artifact_scanner::message_items::SendItem;
use crate::scanner::artifact_scanner::scan_result::GenshinArtifactScanResult;
use crate::scanner::artifact_scanner::GenshinArtifactScannerConfig;

pub struct ArtifactScannerWorkerOutput {
    pub results: Vec<GenshinArtifactScanResult>,
    pub errors: Vec<String>,
}

fn parse_level(s: &str) -> Result<i32> {
    let pos = s.find('+');

    if pos.is_none() {
        let level = s.parse::<i32>()?;
        return anyhow::Ok(level);
    }

    let level = s[pos.unwrap()..].parse::<i32>()?;
    anyhow::Ok(level)
}

fn parse_artifact_level(s: &str, star: usize) -> Result<i32> {
    let level = parse_level(s)?;
    let max_level = match star {
        1 | 2 => 4,
        3 => 12,
        4 => 16,
        5 => 20,
        _ => anyhow::bail!("invalid artifact rarity {star}"),
    };
    if !(0..=max_level).contains(&level) {
        anyhow::bail!("artifact level {level} is outside 0..={max_level}");
    }
    Ok(level)
}

fn infer_max_level_from_main_stat(star: usize, value: &str) -> Option<i32> {
    if star != 5 {
        return None;
    }
    let compact = value.replace(',', "").replace(' ', "");
    matches!(
        compact.as_str(),
        "4780" | "311" | "46.6%" | "58.3%" | "187" | "51.8%" | "31.1%" | "62.2%" | "35.9%"
    )
    .then_some(20)
}

fn get_fast_image_to_text() -> Result<Box<dyn ImageToText<RgbImage> + Send>> {
    let model: Box<dyn ImageToText<RgbImage> + Send> = Box::new(yas_ocr_model!(
        "./models/model_training.onnx",
        "./models/index_2_word.json"
    )?);
    Ok(model)
}

fn get_general_image_to_text() -> Result<Box<dyn ImageToText<RgbImage> + Send>> {
    Ok(Box::new(PPOCRChV4RecInfer::new()?))
}

fn contains_pending_marker(text: &str) -> bool {
    const MARKER: &str = "待激活";
    let compact = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    if compact.contains(MARKER) {
        return true;
    }

    let characters = compact.chars().collect::<Vec<_>>();
    (2..=4).any(|length| {
        characters.windows(length).any(|window| {
            let candidate = window.iter().collect::<String>();
            edit_distance(&candidate, MARKER) <= 1
        })
    })
}

fn is_semantically_blank(text: &str) -> bool {
    !text.chars().any(char::is_alphanumeric)
}

fn next_consecutive_duplicate_count<T: PartialEq>(
    previous: Option<&T>,
    current: &T,
    current_count: i32,
) -> i32 {
    if previous == Some(current) {
        current_count + 1
    } else {
        0
    }
}

fn extract_stat_prefix(text: &str) -> Option<String> {
    let text = text.trim_start_matches(|character: char| {
        character.is_whitespace() || matches!(character, '·' | '•' | '-')
    });
    let plus = text.find('+')?;
    let value_start = plus + 1;
    let mut value_end = value_start;
    for (offset, character) in text[value_start..].char_indices() {
        if character.is_ascii_digit() || matches!(character, '.' | ',' | '%') {
            value_end = value_start + offset + character.len_utf8();
        } else {
            break;
        }
    }
    (value_end > value_start).then(|| text[..value_end].to_string())
}

fn is_plausible_substat(text: &str, pending: bool) -> bool {
    let Some(stat) = ArtifactStat::from_zh_cn_raw(text) else {
        return false;
    };
    let value = stat.value;
    if pending {
        return match stat.name {
            ArtifactStatName::Hp => (200.0..=310.0).contains(&value),
            ArtifactStatName::Atk => (13.0..=20.0).contains(&value),
            ArtifactStatName::Def => (15.0..=24.0).contains(&value),
            ArtifactStatName::HpPercentage | ArtifactStatName::AtkPercentage => {
                (0.040..=0.060).contains(&value)
            },
            ArtifactStatName::DefPercentage => (0.050..=0.075).contains(&value),
            ArtifactStatName::ElementalMastery => (15.0..=24.0).contains(&value),
            ArtifactStatName::Recharge => (0.044..=0.066).contains(&value),
            ArtifactStatName::Critical => (0.026..=0.040).contains(&value),
            ArtifactStatName::CriticalDamage => (0.053..=0.079).contains(&value),
            _ => false,
        };
    }

    match stat.name {
        ArtifactStatName::Hp => value <= 1800.0,
        ArtifactStatName::Atk => value <= 120.0,
        ArtifactStatName::Def => value <= 140.0,
        ArtifactStatName::HpPercentage | ArtifactStatName::AtkPercentage => value <= 0.36,
        ArtifactStatName::DefPercentage => value <= 0.44,
        ArtifactStatName::ElementalMastery => value <= 140.0,
        ArtifactStatName::Recharge => value <= 0.39,
        ArtifactStatName::Critical => value <= 0.24,
        ArtifactStatName::CriticalDamage => value <= 0.47,
        _ => false,
    }
}

fn get_page_locks_from_image(
    window_info: &ArtifactScannerWindowInfo,
    list_image: &RgbImage,
) -> Vec<bool> {
    let mut result = Vec::new();
    let row = window_info.row;
    let col = window_info.col;
    let gap = window_info.item_gap_size;
    let size = window_info.item_size;
    let lock_pos = window_info.lock_pos;

    for r in 0..row {
        if ((gap.height + size.height) * r as f64) as u32 > list_image.height() {
            break;
        }
        for c in 0..col {
            let pos_x = (gap.width + size.width) * c as f64 + lock_pos.x;
            let pos_y = (gap.height + size.height) * r as f64 + lock_pos.y;
            result.push(has_lock_marker(list_image, pos_x as i32, pos_y as i32));
        }
    }
    result
}

/// run in a separate thread, accept captured image and get an artifact
pub struct ArtifactScannerWorker {
    fast_model: Box<dyn ImageToText<RgbImage> + Send>,
    general_model: Box<dyn ImageToText<RgbImage> + Send>,
    catalog: Arc<ArtifactCatalog>,
    window_info: ArtifactScannerWindowInfo,
    config: GenshinArtifactScannerConfig,
}

impl ArtifactScannerWorker {
    pub fn new(
        window_info: ArtifactScannerWindowInfo,
        config: GenshinArtifactScannerConfig,
        catalog: Arc<ArtifactCatalog>,
    ) -> Result<Self> {
        Ok(ArtifactScannerWorker {
            fast_model: get_fast_image_to_text()?,
            general_model: get_general_image_to_text()?,
            catalog,
            window_info,
            config,
        })
    }

    /// the captured_img is a panel of the artifact, the rect is a region of the panel
    fn model_inference(
        &self,
        model: &(dyn ImageToText<RgbImage> + Send),
        rect: Rect<f64>,
        captured_img: &RgbImage,
    ) -> Result<String> {
        let relative_rect = rect.translate(Pos {
            x: -self.window_info.panel_rect.left,
            y: -self.window_info.panel_rect.top,
        });

        let raw_img = captured_img
            .view(
                relative_rect.left as u32,
                relative_rect.top as u32,
                relative_rect.width as u32,
                relative_rect.height as u32,
            )
            .to_image();

        let inference_result = model.image_to_text(&raw_img, false);

        inference_result
    }

    fn fast_inference(&self, rect: Rect<f64>, image: &RgbImage) -> Result<String> {
        self.model_inference(self.fast_model.as_ref(), rect, image)
    }

    fn general_inference(&self, rect: Rect<f64>, image: &RgbImage) -> Result<String> {
        self.model_inference(self.general_model.as_ref(), rect, image)
    }

    fn recognize_slot(&self, image: &RgbImage) -> Result<Option<ArtifactSlot>> {
        let title = self.window_info.title_rect;
        let slot_rect = Rect {
            left: title.left,
            top: title.top + title.height + 8.0,
            width: title.width * 0.6,
            height: title.height * 0.8,
        };
        let text = self.general_inference(slot_rect, image)?;
        let slot = if text.contains("生之花") {
            Some(ArtifactSlot::Flower)
        } else if text.contains("死之羽") {
            Some(ArtifactSlot::Feather)
        } else if text.contains("时之沙") {
            Some(ArtifactSlot::Sand)
        } else if text.contains("空之杯") {
            Some(ArtifactSlot::Goblet)
        } else if text.contains("理之冠") {
            Some(ArtifactSlot::Head)
        } else {
            None
        };
        Ok(slot)
    }

    fn recognize_title(&self, image: &RgbImage) -> Result<String> {
        let fast = self.fast_inference(self.window_info.title_rect, image)?;
        if let Some(matched) = self.catalog.find_piece(&[&fast]) {
            if matched.distance == 0 {
                return Ok(matched.piece_name_zh_cn);
            }
        }
        let general = self.general_inference(self.window_info.title_rect, image)?;
        if let Some(matched) = self.catalog.find_piece(&[&general, &fast]) {
            if matched.distance > 0 {
                warn!(
                    "圣遗物名称 OCR 已校正: fast=`{}`, general=`{}`, catalog=`{}`",
                    fast, general, matched.piece_name_zh_cn
                );
            }
            return Ok(matched.piece_name_zh_cn);
        }
        if let Some(slot) = self.recognize_slot(image)? {
            if let Some(matched) = self.catalog.find_piece_in_slot(&[&general, &fast], &slot) {
                warn!(
                    "圣遗物名称已按部位校正: fast=`{}`, general=`{}`, catalog=`{}`",
                    fast, general, matched.piece_name_zh_cn
                );
                return Ok(matched.piece_name_zh_cn);
            }
        }

        warn!("未知圣遗物名称: fast=`{fast}`, general=`{general}`");
        Ok(if general.is_empty() {
            fast.clone()
        } else {
            general
        })
    }

    fn recognize_equip(&self, image: &RgbImage) -> Result<String> {
        let fast = self.fast_inference(self.window_info.item_equip_rect, image)?;
        if is_semantically_blank(&fast) {
            return Ok(String::new());
        }
        if let Some(matched) = self.catalog.find_character(&[&fast]) {
            if matched.distance == 0 {
                return Ok(format!("{}已装备", matched.character.name_zh_cn()));
            }
        }
        let general = self.general_inference(self.window_info.item_equip_rect, image)?;
        if let Some(matched) = self.catalog.find_character(&[&general, &fast]) {
            return Ok(format!("{}已装备", matched.character.name_zh_cn()));
        }

        warn!("未知装备角色: fast=`{fast}`, general=`{general}`");
        Ok(String::new())
    }

    /// Parse the captured result (of type SendItem) to a scanned artifact
    fn scan_item_image(&self, item: SendItem, lock: bool) -> Result<GenshinArtifactScanResult> {
        let image = &item.panel_image;

        let str_title = self.recognize_title(image)?;
        let str_main_stat_name =
            self.fast_inference(self.window_info.main_stat_name_rect, image)?;
        let str_main_stat_value =
            self.fast_inference(self.window_info.main_stat_value_rect, image)?;

        let str_level = self.fast_inference(self.window_info.level_rect, image)?;
        let level = match parse_artifact_level(&str_level, item.star) {
            Ok(level) => level,
            Err(fast_error) => {
                let general_level = self.general_inference(self.window_info.level_rect, image)?;
                match parse_artifact_level(&general_level, item.star) {
                    Ok(level) => level,
                    Err(general_error) => {
                        if let Some(level) =
                            infer_max_level_from_main_stat(item.star, &str_main_stat_value)
                        {
                            warn!(
                                "inferred max artifact level from main stat value `{}` after level OCR fast=`{}` and general=`{}`",
                                str_main_stat_value, str_level, general_level
                            );
                            level
                        } else {
                            return Err(anyhow::anyhow!(
                                "level OCR failed: fast=`{}` ({fast_error}), general=`{}` ({general_error})",
                                str_level,
                                general_level
                            ));
                        }
                    },
                }
            },
        };

        let str_sub_stat0 = self.fast_inference(self.window_info.sub_stat_1, image)?;
        let str_sub_stat1 = self.fast_inference(self.window_info.sub_stat_2, image)?;
        let str_sub_stat2 = self.fast_inference(self.window_info.sub_stat_3, image)?;
        let mut str_sub_stat3 = self.fast_inference(self.window_info.sub_stat_4, image)?;
        let mut sub_stat_active = [true; 4];
        if item.star == 5 && level < 4 && !str_sub_stat3.is_empty() {
            let general_fourth = self.general_inference(self.window_info.sub_stat_4, image)?;
            if contains_pending_marker(&general_fourth) {
                sub_stat_active[3] = false;
                if let Some(general_stat) = extract_stat_prefix(&general_fourth) {
                    str_sub_stat3 = general_stat;
                }
                info!(
                    "检测到待激活副词条: fast=`{}`, general=`{}`",
                    str_sub_stat3, general_fourth
                );
            }
        }

        let substat_rects = [
            self.window_info.sub_stat_1,
            self.window_info.sub_stat_2,
            self.window_info.sub_stat_3,
            self.window_info.sub_stat_4,
        ];
        let mut substats = [str_sub_stat0, str_sub_stat1, str_sub_stat2, str_sub_stat3];
        for index in 0..substats.len() {
            if is_semantically_blank(&substats[index])
                || is_plausible_substat(&substats[index], !sub_stat_active[index])
            {
                continue;
            }
            let general = self.general_inference(substat_rects[index], image)?;
            if let Some(corrected) = extract_stat_prefix(&general) {
                if is_plausible_substat(&corrected, !sub_stat_active[index]) {
                    warn!(
                        "副词条 OCR 已校正: fast=`{}`, general=`{}`",
                        substats[index], corrected
                    );
                    substats[index] = corrected;
                    continue;
                }
            }
            warn!(
                "副词条 OCR 无法解析，按空词条处理: fast=`{}`, general=`{}`",
                substats[index], general
            );
            substats[index].clear();
        }

        let str_equip = self.recognize_equip(image)?;

        anyhow::Ok(GenshinArtifactScanResult {
            name: str_title,
            main_stat_name: str_main_stat_name,
            main_stat_value: str_main_stat_value,
            sub_stat: substats,
            sub_stat_active,
            level,
            equip: str_equip,
            star: item.star as i32,
            lock,
        })
    }

    pub(crate) fn scan_single(
        &self,
        item: SendItem,
        lock: bool,
    ) -> Result<GenshinArtifactScanResult> {
        self.scan_item_image(item, lock)
    }

    fn get_page_locks(&self, list_image: &RgbImage) -> Vec<bool> {
        get_page_locks_from_image(&self.window_info, list_image)
    }

    pub fn run_pool(
        workers: Vec<Self>,
        rx: Receiver<Option<SendItem>>,
    ) -> JoinHandle<ArtifactScannerWorkerOutput> {
        assert!(!workers.is_empty());
        std::thread::spawn(move || {
            let worker_count = workers.len();
            let is_verbose = workers[0].config.verbose;
            let min_level = workers[0].config.min_level;
            let ignore_dup = workers[0].config.ignore_dup;
            let info = workers[0].window_info.clone();
            let mut job_senders = Vec::with_capacity(worker_count);
            let mut worker_handles = Vec::with_capacity(worker_count);

            for worker in workers {
                let (job_tx, job_rx) = std::sync::mpsc::channel::<(usize, SendItem, bool)>();
                job_senders.push(job_tx);
                worker_handles.push(std::thread::spawn(move || {
                    let mut results = Vec::new();
                    while let Ok((index, item, lock)) = job_rx.recv() {
                        results.push((index, worker.scan_item_image(item, lock)));
                    }
                    results
                }));
            }

            let mut indexed_results = Vec::new();
            let mut locks = Vec::new();
            let mut artifact_index = 0usize;

            for item in rx.into_iter() {
                let item = match item {
                    Some(item) => item,
                    None => break,
                };
                if let Some(list_image) = item.list_image.as_ref() {
                    locks.extend(get_page_locks_from_image(&info, list_image));
                }

                let Some(lock) = locks.get(artifact_index).copied() else {
                    indexed_results.push((
                        artifact_index,
                        Err(anyhow::anyhow!(
                            "item {} has no captured lock state",
                            artifact_index + 1
                        )),
                    ));
                    artifact_index += 1;
                    continue;
                };
                let worker_index = artifact_index % worker_count;
                if job_senders[worker_index]
                    .send((artifact_index, item, lock))
                    .is_err()
                {
                    indexed_results.push((
                        artifact_index,
                        Err(anyhow::anyhow!(
                            "OCR worker {} stopped unexpectedly",
                            worker_index + 1
                        )),
                    ));
                }
                artifact_index += 1;
            }

            drop(job_senders);
            for handle in worker_handles {
                match handle.join() {
                    Ok(mut results) => indexed_results.append(&mut results),
                    Err(_) => indexed_results.push((
                        artifact_index,
                        Err(anyhow::anyhow!("OCR worker thread panicked")),
                    )),
                }
            }
            indexed_results.sort_by_key(|(index, _)| *index);

            let mut results = Vec::new();
            let mut errors = Vec::new();
            let mut hash: HashSet<GenshinArtifactScanResult> = HashSet::new();
            let mut consecutive_dup_count = 0;
            let mut previous_result = None;

            for (index, result) in indexed_results {
                let result = match result {
                    Ok(result) => result,
                    Err(error) => {
                        error!("recognition error: {}", error);
                        errors.push(format!("item {}: {error:#}", index + 1));
                        continue;
                    },
                };
                if is_verbose {
                    info!("{:?}", result);
                }
                if result.level < min_level {
                    break;
                }

                consecutive_dup_count = next_consecutive_duplicate_count(
                    previous_result.as_ref(),
                    &result,
                    consecutive_dup_count,
                );
                previous_result = Some(result.clone());

                if !hash.insert(result.clone()) {
                    warn!("duplicate artifact: {result:#?}");
                }
                results.push(result.clone());

                if consecutive_dup_count >= info.col && !ignore_dup {
                    let message = format!(
                        "item {}: detected {} consecutive duplicate artifacts; page selection may be stale; repeated result: {result:#?}",
                        index + 1,
                        consecutive_dup_count,
                    );
                    error!("{}", message);
                    errors.push(message);
                    break;
                }
            }

            info!(
                "recognition finished: {} artifacts, {} unique",
                results.len(),
                hash.len()
            );
            ArtifactScannerWorkerOutput { results, errors }
        })
    }

    pub fn run(self, rx: Receiver<Option<SendItem>>) -> JoinHandle<ArtifactScannerWorkerOutput> {
        std::thread::spawn(move || {
            let mut results = Vec::new();
            let mut errors = Vec::new();
            let mut hash: HashSet<GenshinArtifactScanResult> = HashSet::new();
            // if too many artifacts are same in consecutive, then an error has occurred
            let mut consecutive_dup_count = 0;
            let mut previous_result = None;

            let is_verbose = self.config.verbose;
            let min_level = self.config.min_level;
            let info = self.window_info.clone();
            // todo remove dump mode to another scanner
            // let dump_mode = false;
            // let model = self.model.clone();
            // let panel_origin = Pos { x: self.window_info.panel_rect.left, y: self.window_info.panel_rect.top };

            let mut locks = Vec::new();
            let mut artifact_index: i32 = 0;

            for item in rx.into_iter() {
                // receiving None, which means the worker should end
                let item = match item {
                    Some(v) => v,
                    None => break,
                };

                // if there is a list image, then parse the lock state
                match item.list_image.as_ref() {
                    Some(v) => {
                        locks = vec![locks, self.get_page_locks(v)].concat();
                    },
                    None => {},
                };

                artifact_index += 1;
                let result = match self.scan_item_image(item, locks[artifact_index as usize - 1]) {
                    Ok(v) => v,
                    Err(e) => {
                        error!("识别错误: {}", e);
                        errors.push(format!("item {}: {e:#}", artifact_index));
                        continue;
                    },
                };

                if is_verbose {
                    info!("{:?}", result);
                }

                if result.level < min_level {
                    info!(
                        "找到满足最低等级要求 {} 的物品({})，准备退出……",
                        min_level, result.level
                    );
                    break;
                }

                consecutive_dup_count = next_consecutive_duplicate_count(
                    previous_result.as_ref(),
                    &result,
                    consecutive_dup_count,
                );
                previous_result = Some(result.clone());

                if hash.contains(&result) {
                    warn!("识别到重复物品: {:#?}", result);
                } else {
                    hash.insert(result.clone());
                }
                results.push(result.clone());

                if consecutive_dup_count >= info.col && !self.config.ignore_dup {
                    let message = format!(
                        "item {}: detected {} consecutive duplicate artifacts; page selection may be stale; repeated result: {result:#?}",
                        artifact_index, consecutive_dup_count,
                    );
                    error!("{}", message);
                    errors.push(message);
                    // token.cancel();
                    break;
                }

                // if token.cancelled() {
                // error!("扫描任务被取消");
                // break;
                // }
            }

            info!(
                "识别结束，物品总数: {}，非重复物品数量: {}",
                results.len(),
                hash.len()
            );

            // progress_bar.finish();
            // MULTI_PROGRESS.remove(&progress_bar);

            ArtifactScannerWorkerOutput { results, errors }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        contains_pending_marker, infer_max_level_from_main_stat, next_consecutive_duplicate_count,
        parse_artifact_level,
    };

    #[test]
    fn rejects_levels_above_the_rarity_cap() {
        assert_eq!(parse_artifact_level("+20", 5).unwrap(), 20);
        assert_eq!(parse_artifact_level("12", 3).unwrap(), 12);
        assert!(parse_artifact_level("87", 5).is_err());
        assert!(parse_artifact_level("+20", 4).is_err());
    }

    #[test]
    fn infers_five_star_max_level_from_known_main_stat_caps() {
        assert_eq!(infer_max_level_from_main_stat(5, "4,780"), Some(20));
        assert_eq!(infer_max_level_from_main_stat(5, "46.6%"), Some(20));
        assert_eq!(infer_max_level_from_main_stat(5, "62.2%"), Some(20));
        assert_eq!(infer_max_level_from_main_stat(4, "46.6%"), None);
        assert_eq!(infer_max_level_from_main_stat(5, "44.6%"), None);
    }

    #[test]
    fn duplicate_streak_only_counts_adjacent_identical_results() {
        assert_eq!(next_consecutive_duplicate_count(Some(&1), &1, 0), 1);
        assert_eq!(next_consecutive_duplicate_count(Some(&1), &1, 7), 8);
        assert_eq!(next_consecutive_duplicate_count(Some(&1), &2, 7), 0);
        assert_eq!(next_consecutive_duplicate_count::<i32>(None, &1, 7), 0);
    }

    #[test]
    fn detects_exact_and_slightly_misread_pending_markers() {
        assert!(contains_pending_marker("暴击伤害+7.8% 待激活"));
        assert!(contains_pending_marker("暴击伤害+7.8%待激话"));
        assert!(contains_pending_marker("暴击伤害+7.8%激活"));
        assert!(!contains_pending_marker("暴击伤害+7.8%"));
    }

    #[test]
    fn punctuation_only_ocr_is_treated_as_blank() {
        assert!(super::is_semantically_blank("..........................."));
        assert!(super::is_semantically_blank(" · … "));
        assert!(!super::is_semantically_blank("桑多涅已装备"));
    }

    #[test]
    fn extracts_pending_stat_without_activation_label() {
        assert_eq!(
            super::extract_stat_prefix("· 防御力+6.6%（待激活）").as_deref(),
            Some("防御力+6.6%")
        );
        assert_eq!(
            super::extract_stat_prefix("攻击力+19待激活").as_deref(),
            Some("攻击力+19")
        );
    }

    #[test]
    fn rejects_impossible_substat_values() {
        assert!(super::is_plausible_substat("防御力+6.6%", true));
        assert!(super::is_plausible_substat("暴击率+10.1%", false));
        assert!(!super::is_plausible_substat("防御力+668", true));
        assert!(!super::is_plausible_substat("暴击率+278", false));
        assert!(!super::is_plausible_substat("攻击1+5388", false));
    }
}
