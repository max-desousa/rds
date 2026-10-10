//! Squarified treemap layout in terminal cells.
//!
//! Weights are the byte sizes of one directory's children, largest first.
//! The rectangles tile `area` exactly. The app nests this from the scan root
//! so each folder keeps its share of the cwd.

use ratatui::layout::Rect;

/// One rectangle per input weight, in the same order.
pub(crate) fn layout(weights: &[u64], area: Rect) -> Vec<Rect> {
    let mut rects = vec![Rect::default(); weights.len()];
    if area.width == 0 || area.height == 0 {
        return rects;
    }
    let items: Vec<(usize, u64)> = weights
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, weight)| *weight > 0)
        .collect();
    if items.is_empty() {
        return rects;
    }
    place(&items, area, &mut rects);
    rects
}

/// `vertical` means items in a row are stacked top to bottom inside a strip
/// cut from the left. Otherwise they sit side by side in a strip cut from the top.
fn place(items: &[(usize, u64)], area: Rect, out: &mut [Rect]) {
    if items.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }
    if items.len() == 1 {
        out[items[0].0] = area;
        return;
    }
    // A one-cell-thick strip has no room to cut a row off. Lay the items along it.
    if area.width == 1 || area.height == 1 {
        place_row(items, area, area.width == 1, out);
        return;
    }

    let total: u64 = items.iter().map(|(_, weight)| *weight).sum();
    let vertical = area.width >= area.height;
    let mut row_len = 1;
    while row_len < items.len() {
        let current = worst_aspect(&items[..row_len], area, total, vertical);
        let with_next = worst_aspect(&items[..=row_len], area, total, vertical);
        if with_next <= current {
            row_len += 1;
        } else {
            break;
        }
    }

    if row_len == items.len() {
        place_row(items, area, vertical, out);
        return;
    }

    let row_weight: u64 = items[..row_len].iter().map(|(_, weight)| *weight).sum();
    let (row_area, rest_area) = split_strip(area, row_weight, total, vertical);
    place_row(&items[..row_len], row_area, vertical, out);
    place(&items[row_len..], rest_area, out);
}

fn place_row(row: &[(usize, u64)], area: Rect, vertical: bool, out: &mut [Rect]) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let weights: Vec<u64> = row.iter().map(|(_, weight)| *weight).collect();
    if vertical {
        let heights = distribute(area.height, &weights);
        let mut y = area.y;
        for ((index, _), height) in row.iter().zip(heights) {
            if height > 0 {
                out[*index] = Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height,
                };
            }
            y = y.saturating_add(height);
        }
    } else {
        let widths = distribute(area.width, &weights);
        let mut x = area.x;
        for ((index, _), width) in row.iter().zip(widths) {
            if width > 0 {
                out[*index] = Rect {
                    x,
                    y: area.y,
                    width,
                    height: area.height,
                };
            }
            x = x.saturating_add(width);
        }
    }
}

fn split_strip(area: Rect, part: u64, whole: u64, vertical: bool) -> (Rect, Rect) {
    if vertical {
        let width = proportional(area.width, part, whole).clamp(1, area.width - 1);
        let row = Rect {
            x: area.x,
            y: area.y,
            width,
            height: area.height,
        };
        let rest = Rect {
            x: area.x + width,
            y: area.y,
            width: area.width - width,
            height: area.height,
        };
        (row, rest)
    } else {
        let height = proportional(area.height, part, whole).clamp(1, area.height - 1);
        let row = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height,
        };
        let rest = Rect {
            x: area.x,
            y: area.y + height,
            width: area.width,
            height: area.height - height,
        };
        (row, rest)
    }
}

/// Largest aspect ratio in the candidate row. Smaller is closer to square.
fn worst_aspect(row: &[(usize, u64)], area: Rect, total: u64, vertical: bool) -> f64 {
    let row_weight: u64 = row.iter().map(|(_, weight)| *weight).sum();
    if row_weight == 0 || total == 0 || area.width == 0 || area.height == 0 {
        return f64::MAX;
    }
    let (thickness, length) = if vertical {
        (
            row_weight as f64 / total as f64 * f64::from(area.width),
            f64::from(area.height),
        )
    } else {
        (
            row_weight as f64 / total as f64 * f64::from(area.height),
            f64::from(area.width),
        )
    };
    if thickness <= f64::EPSILON || length <= f64::EPSILON {
        return f64::MAX;
    }
    let mut worst = 0.0_f64;
    for (_, weight) in row {
        let along = *weight as f64 / row_weight as f64 * length;
        if along <= f64::EPSILON {
            return f64::MAX;
        }
        worst = worst.max((thickness / along).max(along / thickness));
    }
    worst
}

/// Split `total` cells in proportion to `weights`. The parts sum to `total`.
fn distribute(total: u16, weights: &[u64]) -> Vec<u16> {
    let whole: u64 = weights.iter().sum();
    if total == 0 || weights.is_empty() || whole == 0 {
        return vec![0; weights.len()];
    }
    let mut sizes = Vec::with_capacity(weights.len());
    let mut consumed_weight = 0u64;
    let mut consumed_cells = 0u16;
    for weight in weights {
        consumed_weight += *weight;
        let target = proportional(total, consumed_weight, whole);
        let size = target.saturating_sub(consumed_cells);
        sizes.push(size);
        consumed_cells = target;
    }
    sizes
}

fn proportional(total: u16, part: u64, whole: u64) -> u16 {
    if whole == 0 || total == 0 {
        return 0;
    }
    let value = u128::from(part) * u128::from(total) / u128::from(whole);
    u16::try_from(value).unwrap_or(u16::MAX).min(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_tiled(weights: &[u64], area: Rect) {
        let tiles = layout(weights, area);
        assert_eq!(tiles.len(), weights.len());
        let mut owner = vec![None; area.area() as usize];
        for (index, tile) in tiles.iter().enumerate() {
            if weights[index] == 0 {
                assert_eq!(tile.area(), 0, "zero weight produced a tile");
                continue;
            }
            assert!(tile.width > 0 && tile.height > 0 || tile.area() == 0);
            for y in tile.y..tile.y + tile.height {
                for x in tile.x..tile.x + tile.width {
                    assert!(x >= area.x && x < area.right(), "x {x} outside {area:?}");
                    assert!(y >= area.y && y < area.bottom(), "y {y} outside {area:?}");
                    let cell = ((y - area.y) * area.width + (x - area.x)) as usize;
                    assert!(owner[cell].is_none(), "overlap at ({x}, {y})");
                    owner[cell] = Some(index);
                }
            }
        }
        if weights.iter().any(|weight| *weight > 0) {
            assert!(
                owner.iter().all(Option::is_some),
                "gap in {area:?} for {weights:?}"
            );
        }
    }

    #[test]
    fn tiles_cover_the_area_without_overlap() {
        let samples = [
            (vec![1], Rect::new(0, 0, 1, 1)),
            (vec![8, 2], Rect::new(2, 3, 10, 10)),
            (vec![30, 10, 5], Rect::new(0, 0, 15, 6)),
            (vec![5, 4, 3, 2, 1], Rect::new(1, 1, 20, 8)),
            (vec![9, 1, 0, 4], Rect::new(0, 0, 12, 5)),
            (vec![1, 1, 1, 1, 1, 1], Rect::new(0, 0, 3, 2)),
            (vec![100, 1, 1, 1], Rect::new(0, 0, 1, 8)),
            (vec![3, 2, 1], Rect::new(4, 4, 8, 1)),
        ];
        for (weights, area) in samples {
            assert_tiled(&weights, area);
        }
    }

    #[test]
    fn larger_weight_gets_more_cells() {
        let area = Rect::new(0, 0, 10, 10);
        let tiles = layout(&[50, 30, 20], area);
        let areas: Vec<u32> = tiles.iter().map(|tile| tile.area()).collect();
        assert!(areas[0] > areas[1] && areas[1] > areas[2], "{areas:?}");
        assert_eq!(areas.iter().sum::<u32>(), area.area());
        assert!((areas[0] as i32 - 50).abs() <= 15, "{areas:?}");
    }

    #[test]
    fn single_item_fills_the_area() {
        let area = Rect::new(5, 6, 12, 4);
        let tiles = layout(&[42], area);
        assert_eq!(tiles[0], area);
    }

    #[test]
    fn zero_weights_leave_the_area_empty() {
        let tiles = layout(&[0, 0], Rect::new(0, 0, 4, 4));
        assert!(tiles.iter().all(|tile| tile.area() == 0));
    }
}
