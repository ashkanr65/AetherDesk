use ad_stream_protocol::Rectangle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionClass {
    UiText,
    Motion,
    Mixed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassifiedRegion {
    pub rectangle: Rectangle,
    pub class: RegionClass,
    pub edge_score: f32,
    pub changed_ratio: f32,
}

#[derive(Default)]
pub struct RegionClassifier {
    previous: Vec<u8>,
    previous_width: u32,
    previous_height: u32,
}

impl RegionClassifier {
    pub fn classify(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        stride: usize,
        damage: &[Rectangle],
    ) -> Vec<ClassifiedRegion> {
        if width == 0 || height == 0 || stride < width as usize * 4 {
            return Vec::new();
        }
        let required = stride.saturating_mul(height as usize);
        if bgra.len() < required {
            return Vec::new();
        }
        let comparable = self.previous_width == width
            && self.previous_height == height
            && self.previous.len() == required;

        let mut result = Vec::with_capacity(damage.len());
        for requested in damage {
            let Some(rectangle) = clamp_rectangle(*requested, width, height) else {
                continue;
            };
            let mut samples = 0_u64;
            let mut edges = 0_u64;
            let mut changed = 0_u64;
            let y_end = rectangle.y + rectangle.height;
            let x_end = rectangle.x + rectangle.width;
            for y in (rectangle.y..y_end).step_by(2) {
                for x in (rectangle.x..x_end).step_by(2) {
                    let offset = y as usize * stride + x as usize * 4;
                    let luma = pixel_luma(&bgra[offset..offset + 4]);
                    if x + 1 < x_end {
                        let right = pixel_luma(&bgra[offset + 4..offset + 8]);
                        edges += u64::from(luma.abs_diff(right) > 40);
                    }
                    if y + 1 < y_end {
                        let below_offset = offset + stride;
                        let below = pixel_luma(&bgra[below_offset..below_offset + 4]);
                        edges += u64::from(luma.abs_diff(below) > 40);
                    }
                    if comparable {
                        let previous_luma = pixel_luma(&self.previous[offset..offset + 4]);
                        changed += u64::from(luma.abs_diff(previous_luma) > 12);
                    }
                    samples += 1;
                }
            }
            let edge_score = edges as f32 / (samples.max(1) * 2) as f32;
            let changed_ratio = if comparable {
                changed as f32 / samples.max(1) as f32
            } else {
                1.0
            };
            let class = if changed_ratio > 0.18 {
                RegionClass::Motion
            } else if edge_score > 0.12 && changed_ratio < 0.08 {
                RegionClass::UiText
            } else {
                RegionClass::Mixed
            };
            result.push(ClassifiedRegion {
                rectangle,
                class,
                edge_score,
                changed_ratio,
            });
        }

        self.previous.clear();
        self.previous.extend_from_slice(&bgra[..required]);
        self.previous_width = width;
        self.previous_height = height;
        result
    }
}

fn clamp_rectangle(rectangle: Rectangle, width: u32, height: u32) -> Option<Rectangle> {
    let x = rectangle.x.min(width);
    let y = rectangle.y.min(height);
    let x_end = rectangle.x.saturating_add(rectangle.width).min(width);
    let y_end = rectangle.y.saturating_add(rectangle.height).min(height);
    (x_end > x && y_end > y).then_some(Rectangle {
        x,
        y,
        width: x_end - x,
        height: y_end - y,
    })
}

fn pixel_luma(pixel: &[u8]) -> u8 {
    let b = u32::from(pixel[0]);
    let g = u32::from(pixel[1]);
    let r = u32::from(pixel[2]);
    ((29 * b + 150 * g + 77 * r) >> 8) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_motion_between_frames() {
        let mut classifier = RegionClassifier::default();
        let region = Rectangle {
            x: 0,
            y: 0,
            width: 16,
            height: 16,
        };
        let black = vec![0; 16 * 16 * 4];
        classifier.classify(&black, 16, 16, 64, &[region]);
        let white = vec![255; 16 * 16 * 4];
        let result = classifier.classify(&white, 16, 16, 64, &[region]);
        assert_eq!(result[0].class, RegionClass::Motion);
    }

    #[test]
    fn rejects_out_of_bounds_damage() {
        let mut classifier = RegionClassifier::default();
        let frame = vec![0; 4 * 4 * 4];
        let result = classifier.classify(
            &frame,
            4,
            4,
            16,
            &[Rectangle {
                x: 20,
                y: 20,
                width: 1,
                height: 1,
            }],
        );
        assert!(result.is_empty());
    }
}
