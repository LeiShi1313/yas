use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use image::io::Reader as ImageReader;
use image::RgbImage;

use yas::game_info::GameInfoBuilder;
use yas::ocr::{ImageToText, PPOCRChV4RecInfer};
use yas::system_control::SystemControl;
use yas::utils;
use yas::window_info::load_window_info_repo;
use yas_scanner_genshin::artifact::{ArtifactCatalog, ArtifactStat, GenshinArtifact};
use yas_scanner_genshin::export::artifact::GOODFormat;
use yas_scanner_genshin::scanner::{GenshinArtifactScanner, GenshinArtifactScannerConfig};
use yas_scanner_genshin::scanner_controller::repository_layout::{
    GenshinRepositoryScanController, GenshinRepositoryScannerLogicConfig,
};

fn window_info_repository() -> yas::window_info::WindowInfoRepository {
    load_window_info_repo!(
        "../../window_info/windows1600x900.json",
        "../../window_info/windows1280x960.json",
        "../../window_info/windows1440x900.json",
        "../../window_info/windows2100x900.json",
        "../../window_info/windows3440x1440.json",
    )
}

fn build_game_info() -> Result<yas::game_info::GameInfo> {
    GameInfoBuilder::new()
        .add_local_window_name("原神")
        .add_local_window_name("Genshin Impact")
        .add_cloud_window_name("云·原神")
        .build()
}

fn inventory_tab_point(window: yas::positioning::Rect<i32>, x_ratio: f64) -> (i32, i32) {
    (
        window.left + (window.width as f64 * x_ratio) as i32,
        window.top + (window.height as f64 * 0.045) as i32,
    )
}

fn paimon_button_point(window: yas::positioning::Rect<i32>) -> (i32, i32) {
    (
        window.left + (window.width as f64 * 0.028) as i32,
        window.top + (window.height as f64 * 0.055) as i32,
    )
}

fn paimon_bag_tile_point(window: yas::positioning::Rect<i32>) -> (i32, i32) {
    (
        window.left + (window.width as f64 * 0.103) as i32,
        window.top + (window.height as f64 * 0.67) as i32,
    )
}

fn click_point(control: &mut SystemControl, point: (i32, i32)) -> Result<()> {
    control.mouse_move_to(point.0, point.1)?;
    control.mouse_click()
}

#[cfg(windows)]
fn restore_game_foreground(game_info: &yas::game_info::GameInfo) -> Result<()> {
    for _ in 0..3 {
        yas::utils::restore_foreground_window(game_info.window_handle);
        utils::sleep(250);
        if yas::utils::is_foreground_window(game_info.window_handle) {
            return Ok(());
        }
    }
    Err(anyhow!("could not bring Genshin to the foreground"))
}

#[cfg(not(windows))]
fn restore_game_foreground(_game_info: &yas::game_info::GameInfo) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
fn resize_game_client(
    mut game_info: yas::game_info::GameInfo,
    control: &mut SystemControl,
    width: i32,
    height: i32,
) -> Result<yas::game_info::GameInfo> {
    if game_info.window.width == width && game_info.window.height == height {
        return Ok(game_info);
    }

    let resized = yas::utils::resize_client_area(game_info.window_handle as _, width, height)?;
    utils::sleep(1500);
    game_info = build_game_info()?;
    if game_info.window.width == width && game_info.window.height == height {
        return Ok(game_info);
    }

    println!(
        "direct resize produced {}x{} (requested {width}x{height}); retrying after Alt+Enter",
        resized.width, resized.height
    );
    control.key_alt_enter()?;
    utils::sleep(1500);
    yas::utils::resize_client_area(game_info.window_handle as _, width, height)?;
    utils::sleep(1500);
    game_info = build_game_info()?;
    if game_info.window.width != width || game_info.window.height != height {
        return Err(anyhow!(
            "could not set Genshin client size to {width}x{height}; observed {}x{}",
            game_info.window.width,
            game_info.window.height
        ));
    }
    Ok(game_info)
}

#[cfg(not(windows))]
fn resize_game_client(
    game_info: yas::game_info::GameInfo,
    _control: &mut SystemControl,
    width: i32,
    height: i32,
) -> Result<yas::game_info::GameInfo> {
    if game_info.window.width == width && game_info.window.height == height {
        Ok(game_info)
    } else {
        Err(anyhow!(
            "automatic Genshin client resizing is currently supported only on Windows"
        ))
    }
}

#[cfg(windows)]
fn prepare_artifact_page(output_dir: &Path, width: i32, height: i32) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    let mut control = SystemControl::new();
    let game_info = resize_game_client(build_game_info()?, &mut control, width, height)?;
    let config = GenshinArtifactScannerConfig {
        min_star: 1,
        min_level: 0,
        ignore_dup: false,
        verbose: false,
        number: -1,
        artifact_catalog: None,
        no_catalog_update: true,
    };
    let scanner = GenshinArtifactScanner::new(
        &window_info_repository(),
        config,
        GenshinRepositoryScannerLogicConfig::default(),
        game_info.clone(),
    )?;

    println!("fixture client rect: {:?}", game_info.window);
    let mut last_error = match scanner.get_item_count() {
        Ok(count) => {
            let report = serde_json::json!({
                "attempt": 0,
                "clientWidth": game_info.window.width,
                "clientHeight": game_info.window.height,
                "artifactCount": count,
                "navigation": "already-ready",
            });
            fs::write(
                output_dir.join("fixture.json"),
                serde_json::to_string_pretty(&report)? + "\n",
            )?;
            println!(
                "artifact page already ready: count={count} client={}x{}",
                game_info.window.width, game_info.window.height
            );
            return Ok(());
        },
        Err(error) => {
            println!("initial artifact-page verification failed: {error:#}");
            Some(error)
        },
    };
    for attempt in 1..=3 {
        restore_game_foreground(&game_info)?;
        match attempt {
            1 => {
                control.key_click('b')?;
                utils::sleep(1200);
            },
            2 => {
                control.key_escape()?;
                utils::sleep(500);
                control.key_click('b')?;
                utils::sleep(1200);
            },
            3 => {
                click_point(&mut control, paimon_button_point(game_info.window))?;
                utils::sleep(900);
                click_point(&mut control, paimon_bag_tile_point(game_info.window))?;
                utils::sleep(1200);
            },
            _ => unreachable!(),
        }
        if attempt <= 2 {
            for category_key_presses in 0..12 {
                match scanner.get_item_count() {
                    Ok(count) => {
                        let report = serde_json::json!({
                            "attempt": attempt,
                            "clientWidth": game_info.window.width,
                            "clientHeight": game_info.window.height,
                            "artifactCount": count,
                            "navigation": "keyboard",
                            "categoryKey": "q",
                            "categoryKeyPresses": category_key_presses,
                        });
                        fs::write(
                            output_dir.join("fixture.json"),
                            serde_json::to_string_pretty(&report)? + "\n",
                        )?;
                        println!(
                            "artifact page ready: count={count} client={}x{} keyboard-q={category_key_presses}",
                            game_info.window.width, game_info.window.height
                        );
                        return Ok(());
                    },
                    Err(error) => {
                        println!(
                            "fixture keyboard verification after {category_key_presses} Q presses failed: {error:#}"
                        );
                        last_error = Some(error);
                    },
                }
                control.key_click('q')?;
                utils::sleep(600);
            }
        }
        for x_ratio in [0.351, 0.401, 0.301, 0.451] {
            let tab_point = inventory_tab_point(game_info.window, x_ratio);
            println!(
                "fixture attempt {attempt}: inventory tab ratio={x_ratio:.3} at {tab_point:?}"
            );
            click_point(&mut control, tab_point)?;
            utils::sleep(700);

            match scanner.get_item_count() {
                Ok(count) => {
                    let report = serde_json::json!({
                        "attempt": attempt,
                        "clientWidth": game_info.window.width,
                        "clientHeight": game_info.window.height,
                        "artifactCount": count,
                        "artifactTabXRatio": x_ratio,
                    });
                    fs::write(
                        output_dir.join("fixture.json"),
                        serde_json::to_string_pretty(&report)? + "\n",
                    )?;
                    println!(
                        "artifact page ready: count={count} client={}x{} attempt={attempt}",
                        game_info.window.width, game_info.window.height
                    );
                    return Ok(());
                },
                Err(error) => {
                    println!(
                        "fixture attempt {attempt} ratio={x_ratio:.3} verification failed: {error:#}"
                    );
                    last_error = Some(error);
                },
            }
        }
    }

    Err(last_error
        .unwrap_or_else(|| anyhow!("artifact page verification did not run"))
        .context("could not prepare and verify the Genshin artifact inventory page"))
}

#[cfg(not(windows))]
fn prepare_artifact_page(_output_dir: &Path, _width: i32, _height: i32) -> Result<()> {
    Err(anyhow!(
        "automatic Genshin artifact-page setup is currently supported only on Windows"
    ))
}

fn live_probe(output_dir: &Path) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    let game_info = GameInfoBuilder::new()
        .add_local_window_name("原神")
        .add_local_window_name("Genshin Impact")
        .add_cloud_window_name("云·原神")
        .build()?;
    let config = GenshinArtifactScannerConfig {
        min_star: 1,
        min_level: 0,
        ignore_dup: true,
        verbose: true,
        number: 1,
        artifact_catalog: None,
        no_catalog_update: true,
    };
    let panel_path = output_dir.join("panel.png");
    let (scan, _panel) = GenshinArtifactScanner::probe_current(
        &window_info_repository(),
        config,
        game_info,
        Some(&panel_path),
    )?;

    fs::write(output_dir.join("scan.txt"), format!("{scan:#?}\n"))?;

    let catalog = ArtifactCatalog::embedded()?;
    let artifact = GenshinArtifact::from_scan_result(&scan, &catalog)
        .map_err(|()| anyhow!("captured panel OCR could not be resolved through the catalog"))?;
    let good = serde_json::to_string_pretty(&GOODFormat::new(&[artifact]))?;
    let good_path = output_dir.join("good.json");
    fs::write(&good_path, good + "\n")?;

    println!("scan={scan:#?}");
    println!("panel={}", panel_path.display());
    println!("good={}", good_path.display());
    Ok(())
}

fn traverse_probe(output_dir: &Path, number: i32) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    let game_info = GameInfoBuilder::new()
        .add_local_window_name("原神")
        .add_local_window_name("Genshin Impact")
        .add_cloud_window_name("云·原神")
        .build()?;
    let config = GenshinArtifactScannerConfig {
        min_star: 1,
        min_level: 0,
        ignore_dup: false,
        verbose: true,
        number,
        artifact_catalog: None,
        no_catalog_update: true,
    };
    let mut scanner = GenshinArtifactScanner::new(
        &window_info_repository(),
        config,
        GenshinRepositoryScannerLogicConfig::default(),
        game_info,
    )?;
    scanner.set_restore_focus(true);
    let catalog = scanner.catalog();
    let scans = match scanner.scan() {
        Ok(scans) => scans,
        Err(error) => {
            fs::write(
                output_dir.join("error.txt"),
                format!(
                    "captured={}\nabsoluteScrolls={}\n{}\n{error:#}\n",
                    scanner.captured_count(),
                    scanner.absolute_scroll_count(),
                    scanner.scroll_diagnostics()
                ),
            )?;
            return Err(error);
        },
    };
    let captured_count = scanner.captured_count();
    fs::write(output_dir.join("scan.txt"), format!("{scans:#?}\n"))?;

    let mut artifacts = Vec::new();
    let mut panel_identities = HashSet::new();
    let mut duplicate_panel_indices = Vec::new();
    let mut unresolved = Vec::new();
    let mut malformed_substats = Vec::new();
    let mut pending_count = 0;
    for (index, scan) in scans.iter().enumerate() {
        let mut panel_identity = scan.clone();
        panel_identity.lock = false;
        if !panel_identities.insert(panel_identity) {
            duplicate_panel_indices.push(index + 1);
        }
        if scan.sub_stat_active.iter().any(|active| !active) {
            pending_count += 1;
        }
        for (substat_index, raw) in scan.sub_stat.iter().enumerate() {
            if scan.sub_stat_active[substat_index]
                && !raw.is_empty()
                && ArtifactStat::from_zh_cn_raw(raw).is_none()
            {
                malformed_substats.push(format!(
                    "item {} substat {}: `{}`",
                    index + 1,
                    substat_index + 1,
                    raw
                ));
            }
        }
        match GenshinArtifact::from_scan_result(scan, &catalog) {
            Ok(artifact) => artifacts.push(artifact),
            Err(()) => unresolved.push(format!("item {}: {scan:#?}", index + 1)),
        }
    }
    let good = serde_json::to_string_pretty(&GOODFormat::new(&artifacts))?;
    fs::write(output_dir.join("good.json"), good + "\n")?;
    let report = serde_json::json!({
        "scanned": scans.len(),
        "captured": captured_count,
        "absoluteScrolls": scanner.absolute_scroll_count(),
        "exported": artifacts.len(),
        "pendingSubstats": pending_count,
        "duplicatePanelCountIgnoringLock": duplicate_panel_indices.len(),
        "duplicatePanelIndicesIgnoringLock": duplicate_panel_indices,
        "unresolvedCount": unresolved.len(),
        "malformedActiveSubstatCount": malformed_substats.len(),
        "unresolved": unresolved,
        "malformedActiveSubstats": malformed_substats,
    });
    fs::write(
        output_dir.join("audit.json"),
        serde_json::to_string_pretty(&report)? + "\n",
    )?;
    println!(
        "scanned={} exported={} pending={} duplicatePanels={} unresolved={} malformed={}",
        scans.len(),
        artifacts.len(),
        pending_count,
        report["duplicatePanelCountIgnoringLock"],
        report["unresolvedCount"],
        report["malformedActiveSubstatCount"]
    );
    Ok(())
}

fn traverse_capture_probe(output_dir: &Path, number: i32) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    let game_info = GameInfoBuilder::new()
        .add_local_window_name("原神")
        .add_local_window_name("Genshin Impact")
        .add_cloud_window_name("云·原神")
        .build()?;
    let config = GenshinArtifactScannerConfig {
        min_star: 1,
        min_level: 0,
        ignore_dup: true,
        verbose: false,
        number,
        artifact_catalog: None,
        no_catalog_update: true,
    };
    let mut scanner = GenshinArtifactScanner::new(
        &window_info_repository(),
        config,
        GenshinRepositoryScannerLogicConfig::default(),
        game_info,
    )?;
    scanner.set_restore_focus(true);
    let captured = match scanner.capture_only() {
        Ok(captured) => captured,
        Err(error) => {
            fs::write(
                output_dir.join("error.txt"),
                format!(
                    "captured={}\nabsoluteScrolls={}\n{}\n{error:#}\n",
                    scanner.captured_count(),
                    scanner.absolute_scroll_count(),
                    scanner.scroll_diagnostics()
                ),
            )?;
            return Err(error);
        },
    };
    let absolute_scrolls = scanner.absolute_scroll_count();
    fs::write(
        output_dir.join("captured.txt"),
        format!(
            "captured={captured}\nabsoluteScrolls={absolute_scrolls}\n{}\n",
            scanner.scroll_diagnostics()
        ),
    )?;
    println!("captured={captured} absoluteScrolls={absolute_scrolls}");
    Ok(())
}

fn tail_scroll_probe(output_dir: &Path, rows: i32) -> Result<()> {
    fs::create_dir_all(output_dir)?;
    let game_info = GameInfoBuilder::new()
        .add_local_window_name("原神")
        .add_local_window_name("Genshin Impact")
        .add_cloud_window_name("云·原神")
        .build()?;
    let mut controller = GenshinRepositoryScanController::new(
        &window_info_repository(),
        GenshinRepositoryScannerLogicConfig::default(),
        game_info,
        true,
    )?;
    controller.set_restore_focus(true);
    let down_result = controller.scroll_rows(rows);
    let up_result = controller.scroll_rows_up(rows);
    fs::write(
        output_dir.join("result.txt"),
        format!("down={down_result:?}\nup={up_result:?}\n"),
    )?;
    Ok(())
}

fn image_probe(image_path: &str) -> Result<()> {
    let model: Box<dyn ImageToText<RgbImage>> = Box::new(PPOCRChV4RecInfer::new()?);
    let image = ImageReader::open(image_path)?.decode()?;
    let result = model.image_to_text(&image.to_rgb8(), false)?;
    println!("{result}");
    Ok(())
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(|| {
        anyhow!(
            "usage: yas_genshin_playground <image> | --live-probe [output-dir] | \
             --traverse-probe <number> [output-dir] | \
             --traverse-capture-probe <number> [output-dir] | \
             --tail-scroll-probe <rows> [output-dir] | \
             --prepare-artifact-page <width> <height> [output-dir]"
        )
    })?;

    if command == "--prepare-artifact-page" {
        let width = args
            .next()
            .ok_or_else(|| anyhow!("--prepare-artifact-page requires a client width"))?
            .parse::<i32>()?;
        let height = args
            .next()
            .ok_or_else(|| anyhow!("--prepare-artifact-page requires a client height"))?
            .parse::<i32>()?;
        let output_dir = args
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("yas-artifact-page-e2e"));
        prepare_artifact_page(&output_dir, width, height)
    } else if command == "--live-probe" {
        let output_dir = args
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("yas-live-e2e"));
        live_probe(&output_dir)
    } else if command == "--tail-scroll-probe" {
        let rows = args
            .next()
            .ok_or_else(|| anyhow!("--tail-scroll-probe requires a row count"))?
            .parse::<i32>()?;
        let output_dir = args
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("yas-tail-scroll-e2e"));
        tail_scroll_probe(&output_dir, rows)
    } else if command == "--traverse-probe" || command == "--traverse-capture-probe" {
        let number = args
            .next()
            .ok_or_else(|| anyhow!("--traverse-probe requires an item count"))?
            .parse::<i32>()?;
        let output_dir = args
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("yas-traverse-e2e"));
        if command == "--traverse-probe" {
            traverse_probe(&output_dir, number)
        } else {
            traverse_capture_probe(&output_dir, number)
        }
    } else {
        image_probe(&command)
    }
}

fn main() {
    if let Err(error) = run() {
        let message = format!("error: {error:#}\n");
        if let Ok(path) = std::env::var("YAS_E2E_ERROR_FILE") {
            let _ = fs::write(path, &message);
        }
        eprint!("{message}");
        std::process::exit(1);
    }
}
