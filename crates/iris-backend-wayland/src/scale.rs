//! Buffer sizes for integer and fractional output scales.
//!
//! `wp_fractional_scale_v1` reports a numerator over 120. The viewport
//! destination stays in logical pixels. The swapchain uses physical pixels.

/// Round `logical * scale_120 / 120` half away from zero.
///
/// A logical edge of 0 stays 0 so the caller can tell the axis was unset.
/// Any positive logical edge becomes at least 1.
pub(crate) fn physical_size(logical: u32, scale_120: u32) -> u32 {
    if logical == 0 {
        return 0;
    }
    let raw = (u64::from(logical)
        .saturating_mul(u64::from(scale_120))
        .saturating_add(60))
        / 120;
    u32::try_from(raw).unwrap_or(u32::MAX).max(1)
}

/// Swapchain extent. A zero logical axis becomes 1 physical pixel.
pub(crate) fn swapchain_extent(logical_w: u32, logical_h: u32, scale_120: u32) -> (u32, u32) {
    let width = if logical_w == 0 {
        1
    } else {
        physical_size(logical_w, scale_120).max(1)
    };
    let height = if logical_h == 0 {
        1
    } else {
        physical_size(logical_h, scale_120).max(1)
    };
    (width, height)
}

/// Integer buffer scale. Used only when neither viewporter nor fractional scale exists.
pub(crate) fn integer_scale(scale_120: u32) -> u32 {
    (scale_120 / 120).max(1)
}

/// `wl_output` scale factor as a 120ths numerator. Zero and negative become 1.
pub(crate) fn scale_from_factor(factor: i32) -> u32 {
    let factor = u32::try_from(factor.max(1)).unwrap_or(u32::MAX);
    factor.saturating_mul(120)
}

/// Fractional scale as a multiplier. `120` is `1.0`.
pub(crate) fn scale_factor_f64(scale_120: u32) -> f64 {
    f64::from(scale_120.max(1)) / 120.0
}

/// Viewport destination edge. Zero becomes 1. Values above `i32::MAX` saturate.
pub(crate) fn destination_edge(logical: u32) -> i32 {
    let value = logical.max(1);
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// Connector name, or `output-{id}` when the compositor left the name blank.
pub(crate) fn connector_name(name: Option<&str>, id: u32) -> String {
    match name.map(str::trim) {
        Some(trimmed) if !trimmed.is_empty() => trimmed.to_owned(),
        _ => format!("output-{id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{connector_name, physical_size};

    #[test]
    fn physical_pixels_round_half_away_from_zero() {
        assert_eq!(physical_size(800, 180), 1200);
        assert_eq!(physical_size(600, 180), 900);
        assert_eq!(physical_size(100, 180), 150);
        assert_eq!(physical_size(50, 180), 75);
        assert_eq!(physical_size(800, 120), 800);
        assert_eq!(physical_size(1, 1), 1);
        assert_eq!(physical_size(0, 180), 0);
    }

    #[test]
    fn connector_name_falls_back_when_blank() {
        assert_eq!(connector_name(None, 4), "output-4");
        assert_eq!(connector_name(Some(""), 4), "output-4");
        assert_eq!(connector_name(Some("   "), 7), "output-7");
        assert_eq!(connector_name(Some(" DP-1 "), 1), "DP-1");
        assert_eq!(connector_name(Some("eDP-1"), 0), "eDP-1");
    }
}
