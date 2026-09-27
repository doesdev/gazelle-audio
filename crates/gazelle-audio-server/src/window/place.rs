//! Where a window goes on the monitors there are now, which may not be the monitors there were.
//!
//! A remembered position is only worth restoring if the window would be on a screen. A monitor that
//! has been unplugged, or a laptop taken away from its desk, leaves a remembered position off every
//! screen, and a frameless always-on-top widget there is lost for good: nothing to drag, nothing in
//! the taskbar. So a window that would not show enough of itself is put somewhere that does.
//!
//! All in physical pixels, as Windows reports monitors and positions, and all plain data, so it is
//! tested without a desktop.

/// A monitor, as the window thread reads it from Tao.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub name: Option<String>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

impl Monitor {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && i64::from(x) < i64::from(self.x) + i64::from(self.width) && i64::from(y) < i64::from(self.y) + i64::from(self.height)
    }

    /// How much of a rectangle is on this monitor, as a width and a height.
    fn overlap(&self, rect: Rect) -> (i64, i64) {
        let left = i64::from(rect.x.max(self.x));
        let top = i64::from(rect.y.max(self.y));
        let right = (i64::from(rect.x) + i64::from(rect.width)).min(i64::from(self.x) + i64::from(self.width));
        let bottom = (i64::from(rect.y) + i64::from(rect.height)).min(i64::from(self.y) + i64::from(self.height));
        ((right - left).max(0), (bottom - top).max(0))
    }
}

/// A window's outer rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// How much of a window has to be on one monitor for its remembered place to be kept: enough to see
/// it and take hold of it.
pub const VISIBLE: (i64, i64) = (48, 32);
/// How far from the monitor's edges a window put back on screen goes.
pub const MARGIN: i32 = 24;

/// Where a window put back on screen goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// The middle of the primary monitor: the app's window.
    Centre,
    /// The top-right corner of the primary monitor, clear of the edges: the widget.
    TopRight,
}

/// The primary monitor, else the first. `None` only when there are none at all.
pub fn primary(monitors: &[Monitor]) -> Option<&Monitor> {
    monitors.iter().find(|m| m.primary).or_else(|| monitors.first())
}

/// Where to put a window whose remembered outer rectangle is `rect`: there, if enough of it is on a
/// monitor, else on the primary monitor as `fallback` says. With no monitors to go by, where it was.
pub fn keep_on_screen(rect: Rect, monitors: &[Monitor], fallback: Fallback) -> (i32, i32) {
    let seen = monitors.iter().any(|m| {
        let (w, h) = m.overlap(rect);
        w >= VISIBLE.0.min(i64::from(rect.width)) && h >= VISIBLE.1.min(i64::from(rect.height))
    });
    if seen {
        return (rect.x, rect.y);
    }
    let Some(screen) = primary(monitors) else { return (rect.x, rect.y) };
    let fit = |space: u32, size: u32| i32::try_from(space.saturating_sub(size)).unwrap_or(0);
    match fallback {
        Fallback::Centre => (screen.x + fit(screen.width, rect.width) / 2, screen.y + fit(screen.height, rect.height) / 2),
        Fallback::TopRight => (screen.x + (fit(screen.width, rect.width) - MARGIN).max(0), screen.y + MARGIN.min(fit(screen.height, rect.height))),
    }
}

/// The monitor the hub opens on: the one it was last on, by name, else the one holding the point
/// it was last at (a name Windows reshuffled), else the primary monitor. An index into `monitors`.
pub fn monitor_for(name: Option<&str>, point: Option<(i32, i32)>, monitors: &[Monitor]) -> Option<usize> {
    name.and_then(|name| monitors.iter().position(|m| m.name.as_deref() == Some(name)))
        .or_else(|| point.and_then(|(x, y)| monitors.iter().position(|m| m.contains(x, y))))
        .or_else(|| monitors.iter().position(|m| m.primary))
        .or(if monitors.is_empty() { None } else { Some(0) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(name: &str, x: i32, y: i32, width: u32, height: u32, primary: bool) -> Monitor {
        Monitor { name: Some(name.into()), x, y, width, height, primary }
    }

    /// A laptop screen at 150 % on the left and a 4K monitor to its right, the laptop primary.
    fn desk() -> Vec<Monitor> {
        vec![monitor(r"\\.\DISPLAY1", 0, 0, 2880, 1800, true), monitor(r"\\.\DISPLAY2", 2880, -200, 3840, 2160, false)]
    }

    const WIDGET: (u32, u32) = (480, 210);

    fn widget_at(x: i32, y: i32) -> Rect {
        Rect { x, y, width: WIDGET.0, height: WIDGET.1 }
    }

    #[test]
    fn a_window_on_a_monitor_that_is_still_there_stays_where_it_was() {
        assert_eq!(keep_on_screen(widget_at(6000, 100), &desk(), Fallback::TopRight), (6000, 100), "on the second monitor");
        assert_eq!(keep_on_screen(widget_at(-400, 900), &desk(), Fallback::TopRight), (-400, 900), "partly off the left edge, still easy to take hold of");
        assert_eq!(keep_on_screen(widget_at(2700, -100), &desk(), Fallback::TopRight), (2700, -100), "across the two");
    }

    #[test]
    fn a_widget_left_on_an_unplugged_monitor_comes_back_to_the_primary_ones_top_right() {
        let laptop = vec![desk()[0].clone()];
        assert_eq!(keep_on_screen(widget_at(6000, 100), &laptop, Fallback::TopRight), (2880 - 480 - MARGIN, MARGIN));
        // A sliver is not enough to take hold of.
        assert_eq!(keep_on_screen(widget_at(2880 - 20, 100), &laptop, Fallback::TopRight), (2880 - 480 - MARGIN, MARGIN));
    }

    #[test]
    fn the_main_window_comes_back_to_the_middle_of_the_primary_monitor() {
        let laptop = vec![desk()[0].clone()];
        let main = Rect { x: 4000, y: 300, width: 1920, height: 1230 };
        assert_eq!(keep_on_screen(main, &laptop, Fallback::Centre), ((2880 - 1920) / 2, (1800 - 1230) / 2));
        // Larger than the screen: its corner at the screen's.
        assert_eq!(keep_on_screen(Rect { width: 4000, ..main }, &laptop, Fallback::Centre), (0, (1800 - 1230) / 2));
    }

    #[test]
    fn with_no_monitors_to_go_by_nothing_moves() {
        assert_eq!(keep_on_screen(widget_at(6000, 100), &[], Fallback::TopRight), (6000, 100));
        assert_eq!(monitor_for(Some("x"), Some((1, 1)), &[]), None);
    }

    #[test]
    fn the_hub_opens_on_its_monitor_by_name_then_by_place_then_on_the_primary() {
        let monitors = desk();
        assert_eq!(monitor_for(Some(r"\\.\DISPLAY2"), None, &monitors), Some(1));
        // Windows renamed it, but it is still where it was.
        assert_eq!(monitor_for(Some(r"\\.\DISPLAY9"), Some((2880, -200)), &monitors), Some(1));
        // Gone altogether: the primary.
        assert_eq!(monitor_for(Some(r"\\.\DISPLAY9"), Some((9000, 0)), &monitors), Some(0));
        assert_eq!(monitor_for(None, None, &monitors), Some(0), "a first run is on the primary");
        // No monitor says it is primary: the first.
        let unmarked: Vec<Monitor> = monitors.into_iter().map(|m| Monitor { primary: false, ..m }).collect();
        assert_eq!(monitor_for(None, Some((-5000, 0)), &unmarked), Some(0));
    }
}
