use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use crate::game_info::{Platform, ResolutionFamily, UI};
use crate::positioning::{Pos, Scalable, Size};

use crate::window_info::WindowInfoType;

/// Maps a window-info-key to a list of entries
/// where entries consist of a size where the value is recorded, and accordingly a value
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WindowInfoRepository {
    /// window info key -> (window size, ui, platform)
    pub data: HashMap<String, HashMap<(Size<usize>, UI, Platform), WindowInfoType>>,
}

impl WindowInfoRepository {
    pub fn new() -> WindowInfoRepository {
        WindowInfoRepository {
            data: HashMap::new(),
        }
    }

    pub fn add(&mut self, name: &str, size: Size<usize>, ui: UI, platform: Platform, value: WindowInfoType) {
        self.data
            .entry(String::from(name))
            .or_insert(HashMap::new())
            .insert((size, ui, platform), value);
    }

    pub fn add_pos(&mut self, name: &str, size: Size<usize>, ui: UI, platform: Platform, value: Pos<f64>) {
        self.data
            .entry(String::from(name))
            .or_insert(HashMap::new())
            .insert((size, ui, platform), WindowInfoType::Pos(value));
    }

    pub fn merge_inplace(&mut self, other: &WindowInfoRepository) {
        for (key, data) in other.data.iter() {
            if self.data.contains_key(key) {
                for (resolution, value) in data.iter() {
                    self.data.get_mut(key).unwrap().insert(resolution.clone(), value.clone());
                }
            } else {
                self.data.insert(key.clone(), data.clone());
            }
        }
    }

    pub fn merge(&self, other: &WindowInfoRepository) -> WindowInfoRepository {
        let mut result = self.clone();
        result.merge_inplace(other);
        result
    }

    /// Get window info by name and size
    /// if name or resolution does not exist, then return None
    pub fn get_exact<T>(&self, name: &str, window_size: Size<usize>, ui: UI, platform: Platform) -> Option<T> where WindowInfoType: TryInto<T> {
        if self.data.contains_key(name) &&
          self.data[name].contains_key(&(window_size, ui, platform)) {
            return self.data[name][&(window_size, ui, platform)].try_into().ok();
        }

        None
    }

    /// Get window info by name and size
    /// if window size does not exists exactly, this function will search for the same resolution family and scale the result
    pub fn get_auto_scale<T>(
        &self,
        name: &str,
        window_size: Size<usize>,
        ui: UI,
        platform: Platform,
    ) -> Option<T>
    where
        WindowInfoType: TryInto<T>,
    {
        let entries = self.data.get(name)?;
        if let Some(value) = entries.get(&(window_size, ui, platform)) {
            return (*value).try_into().ok();
        }

        let family = ResolutionFamily::new(window_size)?;
        let ((source_size, _, _), value) = entries
            .iter()
            .filter(|((size, source_ui, source_platform), _)| {
                *source_ui == ui
                    && *source_platform == platform
                    && ResolutionFamily::new(*size) == Some(family)
            })
            .min_by(|((left, _, _), _), ((right, _, _), _)| {
                let left_scale = window_size.width as f64 / left.width as f64;
                let right_scale = window_size.width as f64 / right.width as f64;
                left_scale
                    .ln()
                    .abs()
                    .total_cmp(&right_scale.ln().abs())
                    .then_with(|| left.width.cmp(&right.width))
                    .then_with(|| left.height.cmp(&right.height))
            })?;
        let factor = window_size.width as f64 / source_size.width as f64;
        value.scale(factor).try_into().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::WindowInfoRepository;
    use crate::game_info::{Platform, UI};
    use crate::positioning::{Pos, Size};
    use crate::window_info::WindowInfoType;

    #[test]
    fn auto_scale_chooses_the_nearest_profile_deterministically() {
        let mut repository = WindowInfoRepository::new();
        repository.add(
            "row-count",
            Size {
                width: 1280,
                height: 720,
            },
            UI::Desktop,
            Platform::Windows,
            WindowInfoType::InvariantInt(1),
        );
        repository.add(
            "row-count",
            Size {
                width: 1600,
                height: 900,
            },
            UI::Desktop,
            Platform::Windows,
            WindowInfoType::InvariantInt(2),
        );

        let value: i32 = repository
            .get_auto_scale(
                "row-count",
                Size {
                    width: 1366,
                    height: 768,
                },
                UI::Desktop,
                Platform::Windows,
            )
            .unwrap();
        assert_eq!(value, 1);
    }

    #[test]
    fn auto_scale_supports_rounded_aspect_ratios() {
        let mut repository = WindowInfoRepository::new();
        repository.add_pos(
            "position",
            Size {
                width: 1600,
                height: 900,
            },
            UI::Desktop,
            Platform::Windows,
            Pos { x: 100.0, y: 50.0 },
        );

        let value: Pos<f64> = repository
            .get_auto_scale(
                "position",
                Size {
                    width: 1366,
                    height: 768,
                },
                UI::Desktop,
                Platform::Windows,
            )
            .unwrap();
        assert!((value.x - 85.375).abs() < 0.001);
        assert!((value.y - 42.6875).abs() < 0.001);
    }

    #[test]
    fn auto_scale_preserves_row_counts_for_each_layout_family() {
        let mut repository = WindowInfoRepository::new();
        for (size, rows) in [
            ((1600, 900), 5),
            ((1440, 900), 6),
            ((1280, 960), 7),
            ((3440, 1440), 5),
        ] {
            repository.add(
                "row-count",
                Size {
                    width: size.0,
                    height: size.1,
                },
                UI::Desktop,
                Platform::Windows,
                WindowInfoType::InvariantInt(rows),
            );
        }

        for (size, expected_rows) in [
            ((1366, 768), 5),
            ((1920, 1200), 6),
            ((1024, 768), 7),
            ((2560, 1080), 5),
        ] {
            let rows: i32 = repository
                .get_auto_scale(
                    "row-count",
                    Size {
                        width: size.0,
                        height: size.1,
                    },
                    UI::Desktop,
                    Platform::Windows,
                )
                .unwrap();
            assert_eq!(rows, expected_rows, "unexpected row count for {size:?}");
        }
    }
}
