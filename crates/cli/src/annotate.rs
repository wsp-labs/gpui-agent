//! Draws numbered boxes on a screenshot, one per snapshot ref, for models that read images.

use crate::snapshot::Snapshot;
use image::{Rgba, RgbaImage};

const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

const BOX: Rgba<u8> = Rgba([255, 0, 102, 255]);
const LABEL_TEXT: Rgba<u8> = Rgba([255, 255, 255, 255]);

fn put(image: &mut RgbaImage, x: i64, y: i64, color: Rgba<u8>) {
    if x >= 0 && y >= 0 && (x as u32) < image.width() && (y as u32) < image.height() {
        image.put_pixel(x as u32, y as u32, color);
    }
}

fn outline(image: &mut RgbaImage, x0: i64, y0: i64, x1: i64, y1: i64, thickness: i64) {
    for t in 0..thickness {
        for x in x0..=x1 {
            put(image, x, y0 + t, BOX);
            put(image, x, y1 - t, BOX);
        }
        for y in y0..=y1 {
            put(image, x0 + t, y, BOX);
            put(image, x1 - t, y, BOX);
        }
    }
}

fn label(image: &mut RgbaImage, x: i64, y: i64, number: usize, scale: i64) {
    let text = number.to_string();
    let width = text.len() as i64 * 4 * scale + scale;
    let height = 7 * scale;
    for dy in 0..height {
        for dx in 0..width {
            put(image, x + dx, y + dy, BOX);
        }
    }
    for (n, c) in text.chars().enumerate() {
        let glyph = DIGITS[c.to_digit(10).unwrap_or(0) as usize];
        let left = x + scale + n as i64 * 4 * scale;
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            put(
                                image,
                                left + col * scale + sx,
                                y + scale + row as i64 * scale + sy,
                                LABEL_TEXT,
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Draws each entry's box with its ref number. Returns the legend, one line per box.
pub fn annotate(image: &mut RgbaImage, snapshot: &Snapshot) -> Vec<String> {
    let scale = if snapshot.width > 0. {
        image.width() as f32 / snapshot.width
    } else {
        1.
    };
    let thickness = scale.round().max(1.) as i64;
    let text_scale = (scale * 2.).round().max(2.) as i64;
    let mut legend = Vec::new();
    for entry in &snapshot.entries {
        let number: usize = entry.reference.trim_start_matches('e').parse().unwrap_or(0);
        let v = entry.visible;
        if v.is_empty() {
            continue;
        }
        let x0 = (v.x * scale).round() as i64;
        let y0 = (v.y * scale).round() as i64;
        let x1 = ((v.x + v.w) * scale).round() as i64 - 1;
        let y1 = ((v.y + v.h) * scale).round() as i64 - 1;
        outline(image, x0, y0, x1, y1, thickness);
        label(image, x0, y0, number, text_scale);
        legend.push(format!(
            "[{number}] @{} {} {:?}",
            entry.reference, entry.role, entry.name
        ));
    }
    legend
}
