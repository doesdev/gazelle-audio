//! Encoding fit: how a field's raw byte relates to the UI value. Models are tried
//! in order (linear, signed two's complement, dB, monotonic table) and the first that
//! reproduces every observed raw value after rounding wins. Non-numeric UI values get an enum.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "model")]
pub enum Model {
    /// `raw = round(scale · ui + offset)`, raw read as unsigned.
    Linear { scale: f64, offset: f64 },
    /// `raw = round(scale · ui + offset)`, raw read as a two's-complement `i8`.
    Signed { scale: f64, offset: f64 },
    /// `raw = round(scale · 10^(ui / 20) + offset)`, raw read as unsigned.
    Db { scale: f64, offset: f64 },
    /// No formula fits; raw values rise or fall monotonically with the UI value.
    MonotonicTable { entries: Vec<(String, u8)> },
    /// UI values are not numbers, or no ordering fits.
    Table { entries: Vec<(String, u8)> },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Encoding {
    #[serde(flatten)]
    pub model: Model,
    /// Largest `|predicted - raw|` before rounding; 0 for tables.
    pub residual_max: f64,
}

/// The first number in a UI value: `"-6 dB"` → -6, `"+3.5"` → 3.5, `"off"` → `None`.
pub fn ui_number(value: &str) -> Option<f64> {
    let start = value.find(|c: char| c.is_ascii_digit() || c == '-' || c == '+' || c == '.')?;
    let rest = &value[start..];
    let end = rest
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))))
        .map_or(rest.len(), |(i, _)| i);
    rest[..end].parse().ok()
}

/// Fits `(UI value, raw)` pairs, one per distinct UI value.
pub fn fit(values: &[(String, u8)]) -> Encoding {
    let table = || values.to_vec();
    let points: Option<Vec<(f64, u8)>> = values.iter().map(|(v, raw)| Some((ui_number(v)?, *raw))).collect();
    let Some(points) = points.filter(|p| distinct_x(p) >= 2) else {
        return Encoding { model: Model::Table { entries: table() }, residual_max: 0.0 };
    };
    let unsigned: Vec<(f64, f64)> = points.iter().map(|&(x, r)| (x, r as f64)).collect();
    let signed: Vec<(f64, f64)> = points.iter().map(|&(x, r)| (x, r as i8 as f64)).collect();
    let db: Vec<(f64, f64)> = points.iter().map(|&(x, r)| (10f64.powf(x / 20.0), r as f64)).collect();
    if let Some((scale, offset, residual_max)) = exact_line(&unsigned) {
        return Encoding { model: Model::Linear { scale, offset }, residual_max };
    }
    if points.iter().any(|&(_, r)| r >= 0x80) {
        if let Some((scale, offset, residual_max)) = exact_line(&signed) {
            return Encoding { model: Model::Signed { scale, offset }, residual_max };
        }
    }
    if let Some((scale, offset, residual_max)) = exact_line(&db) {
        return Encoding { model: Model::Db { scale, offset }, residual_max };
    }
    let mut sorted = points.clone();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let rising = sorted.windows(2).all(|w| w[0].1 < w[1].1);
    let falling = sorted.windows(2).all(|w| w[0].1 > w[1].1);
    let model = if rising || falling { Model::MonotonicTable { entries: table() } } else { Model::Table { entries: table() } };
    Encoding { model, residual_max: 0.0 }
}

fn distinct_x(points: &[(f64, u8)]) -> usize {
    let mut xs: Vec<f64> = points.iter().map(|p| p.0).collect();
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    xs.len()
}

/// Least-squares line through `(x, y)`, accepted only if rounding reproduces every `y`.
/// Returns `(scale, offset, residual_max)`.
fn exact_line(points: &[(f64, f64)]) -> Option<(f64, f64, f64)> {
    let n = points.len() as f64;
    let (sx, sy) = points.iter().fold((0.0, 0.0), |(sx, sy), &(x, y)| (sx + x, sy + y));
    let (mx, my) = (sx / n, sy / n);
    let sxx: f64 = points.iter().map(|&(x, _)| (x - mx) * (x - mx)).sum();
    if sxx.abs() < f64::EPSILON {
        return None;
    }
    let sxy: f64 = points.iter().map(|&(x, y)| (x - mx) * (y - my)).sum();
    let scale = sxy / sxx;
    let offset = my - scale * mx;
    let mut residual_max: f64 = 0.0;
    for &(x, y) in points {
        let predicted = scale * x + offset;
        if predicted.round() != y {
            return None;
        }
        residual_max = residual_max.max((predicted - y).abs());
    }
    Some((scale, offset, residual_max))
}
