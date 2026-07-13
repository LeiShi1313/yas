use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use image::io::Reader as ImageReader;
use image::RgbImage;

use yas::game_info::GameInfoBuilder;
use yas::ocr::{ImageToText, PPOCRChV4RecInfer};
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
    let mut unresolved = Vec::new();
    let mut malformed_substats = Vec::new();
    let mut pending_count = 0;
    for (index, scan) in scans.iter().enumerate() {
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
        "scanned={} exported={} pending={} unresolved={} malformed={}",
        scans.len(),
        artifacts.len(),
        pending_count,
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

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(|| {
        anyhow!(
            "usage: yas_genshin_playground <image> | --live-probe [output-dir] | \
             --traverse-probe <number> [output-dir] | \
             --traverse-capture-probe <number> [output-dir] | \
             --tail-scroll-probe <rows> [output-dir]"
        )
    })?;

    if command == "--live-probe" {
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
