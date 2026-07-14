use image::{Rgb, RgbImage};
use yas::utils::color_distance;

const LOCK_MARKER_COLOR: Rgb<u8> = Rgb([255, 138, 117]);
const LOCK_MARKER_COLOR_DISTANCE: usize = 30;

pub(crate) fn has_lock_marker(image: &RgbImage, center_x: i32, center_y: i32) -> bool {
    for dx in -1..1 {
        for dy in -10..10 {
            let x = center_x + dx;
            let y = center_y + dy;
            if x < 0 || y < 0 || x >= image.width() as i32 || y >= image.height() as i32 {
                continue;
            }
            if color_distance(image.get_pixel(x as u32, y as u32), &LOCK_MARKER_COLOR)
                < LOCK_MARKER_COLOR_DISTANCE
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use image::{Rgb, RgbImage};

    use super::has_lock_marker;

    #[test]
    fn detects_the_lock_marker_color_near_the_expected_position() {
        let mut image = RgbImage::new(20, 30);
        image.put_pixel(9, 20, Rgb([255, 138, 117]));

        assert!(has_lock_marker(&image, 10, 15));
    }

    #[test]
    fn accepts_small_color_variation_but_rejects_unrelated_pixels() {
        let mut image = RgbImage::new(20, 30);
        image.put_pixel(10, 15, Rgb([252, 140, 119]));
        assert!(has_lock_marker(&image, 10, 15));

        image.put_pixel(10, 15, Rgb([255, 255, 255]));
        assert!(!has_lock_marker(&image, 10, 15));
    }

    #[test]
    fn clips_the_search_region_at_image_edges() {
        let mut image = RgbImage::new(2, 2);
        image.put_pixel(0, 0, Rgb([255, 138, 117]));

        assert!(has_lock_marker(&image, 0, 0));
    }
}
