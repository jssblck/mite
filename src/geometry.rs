use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn long_side(self) -> u32 {
        self.width.max(self.height)
    }

    pub fn scaled_to_long_side(self, max_long_side: u32) -> Self {
        if self.is_empty() || max_long_side == 0 || self.long_side() <= max_long_side {
            return self;
        }

        let scale = max_long_side as f32 / self.long_side() as f32;
        Self {
            width: ((self.width as f32 * scale).round() as u32).max(1),
            height: ((self.height as f32 * scale).round() as u32).max(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub size: Size,
}

impl ScreenRect {
    pub const fn new(x: i32, y: i32, size: Size) -> Self {
        Self { x, y, size }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Build a rect from its left/top/right/bottom edges. Width and height
    /// clamp to zero when the edges cross: the result is a zero-size rect
    /// anchored at (left, top), not a rect over the swapped span, so callers
    /// that can produce inverted edges must treat a zero-size result as
    /// degenerate rather than positioned.
    pub fn from_edges(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            x: left,
            y: top,
            width: (right - left).max(0.0),
            height: (bottom - top).max(0.0),
        }
    }

    pub fn right(self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    /// Whether the point `(x, y)` lies inside the rectangle, treating the right
    /// and bottom edges as exclusive (half-open) so adjacent rects don't both
    /// claim a shared edge.
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn area(self) -> f32 {
        self.width.max(0.0) * self.height.max(0.0)
    }

    pub fn scale(self, sx: f32, sy: f32) -> Self {
        Self {
            x: self.x * sx,
            y: self.y * sy,
            width: self.width * sx,
            height: self.height * sy,
        }
    }

    pub fn clamp_to(self, size: Size) -> Self {
        let x = self.x.clamp(0.0, size.width as f32);
        let y = self.y.clamp(0.0, size.height as f32);
        let right = self.right().clamp(0.0, size.width as f32);
        let bottom = self.bottom().clamp(0.0, size.height as f32);

        Self {
            x,
            y,
            width: (right - x).max(0.0),
            height: (bottom - y).max(0.0),
        }
    }

    pub fn iou(self, other: Self) -> f32 {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        let intersection = Rect::new(left, top, right - left, bottom - top).area();
        if intersection <= 0.0 {
            return 0.0;
        }

        intersection / (self.area() + other.area() - intersection)
    }

    pub fn quantized_key(self, bucket_px: f32) -> (i32, i32, i32, i32) {
        let bucket = bucket_px.max(1.0);
        (
            (self.x / bucket).round() as i32,
            (self.y / bucket).round() as i32,
            (self.width / bucket).round() as i32,
            (self.height / bucket).round() as i32,
        )
    }
}

pub fn sort_reading_order(rects: &mut [Rect]) {
    rects.sort_by(|a, b| {
        a.y.total_cmp(&b.y)
            .then_with(|| a.x.total_cmp(&b.x))
            .then_with(|| a.width.total_cmp(&b.width))
            .then_with(|| a.height.total_cmp(&b.height))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_size_to_long_side_without_stretching() {
        assert_eq!(
            Size::new(3840, 2160).scaled_to_long_side(960),
            Size::new(960, 540)
        );
    }

    #[test]
    fn contains_treats_right_and_bottom_edges_as_exclusive() {
        let r = Rect::new(10.0, 20.0, 30.0, 40.0); // x:[10,40), y:[20,60)
        assert!(r.contains(10.0, 20.0)); // top-left corner is inside
        assert!(r.contains(25.0, 40.0));
        assert!(!r.contains(40.0, 30.0)); // right edge excluded
        assert!(!r.contains(25.0, 60.0)); // bottom edge excluded
        assert!(!r.contains(9.0, 30.0));
    }

    #[test]
    fn computes_iou() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 0.0, 10.0, 10.0);
        assert!((a.iou(b) - 0.33333334).abs() < 0.001);
    }

    #[test]
    fn sorts_reading_order_by_rows_then_x() {
        let mut rects = [
            Rect::new(100.0, 50.0, 20.0, 20.0),
            Rect::new(10.0, 10.0, 20.0, 20.0),
            Rect::new(70.0, 12.0, 20.0, 20.0),
        ];

        sort_reading_order(&mut rects);
        assert_eq!(rects[0].x, 10.0);
        assert_eq!(rects[1].x, 70.0);
        assert_eq!(rects[2].x, 100.0);
    }
}
