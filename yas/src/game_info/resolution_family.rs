use serde::{Deserialize, Serialize};
use crate::positioning::Size;

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub enum ResolutionFamily {
    // PC
    Windows43x18,
    Windows7x3,
    Windows16x9,
    Windows8x5,
    Windows4x3,
    // Mobile
    MacOS8x5,
}

impl ResolutionFamily {
    const MAX_ASPECT_RELATIVE_ERROR: f64 = 0.015;

    pub fn new(size: Size<usize>) -> Option<Self> {
        if size.width == 0 || size.height == 0 {
            return None;
        }

        let actual = size.width as f64 / size.height as f64;
        let candidates = [
            (ResolutionFamily::Windows43x18, 43.0 / 18.0),
            (ResolutionFamily::Windows7x3, 7.0 / 3.0),
            (ResolutionFamily::Windows16x9, 16.0 / 9.0),
            (ResolutionFamily::Windows8x5, 8.0 / 5.0),
            (ResolutionFamily::Windows4x3, 4.0 / 3.0),
        ];
        let (family, expected) = candidates
            .into_iter()
            .min_by(|left, right| (actual - left.1).abs().total_cmp(&(actual - right.1).abs()))?;
        let relative_error = (actual - expected).abs() / expected;
        (relative_error <= Self::MAX_ASPECT_RELATIVE_ERROR).then_some(family)
    }
}

#[cfg(test)]
mod tests {
    use super::ResolutionFamily;
    use crate::positioning::Size;

    #[test]
    fn recognizes_rounded_common_resolutions() {
        assert_eq!(
            ResolutionFamily::new(Size {
                width: 1366,
                height: 768,
            }),
            Some(ResolutionFamily::Windows16x9)
        );
        assert_eq!(
            ResolutionFamily::new(Size {
                width: 2560,
                height: 1080,
            }),
            Some(ResolutionFamily::Windows43x18)
        );
    }

    #[test]
    fn rejects_layouts_far_from_a_calibrated_family() {
        assert_eq!(
            ResolutionFamily::new(Size {
                width: 1000,
                height: 700,
            }),
            None
        );
    }
}
