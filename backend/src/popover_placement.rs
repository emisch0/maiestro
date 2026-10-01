//! Where the menu-bar popover goes on screen (#175).
//!
//! macOS has a single global coordinate space measured in *points* (logical
//! px). tao and tray-icon each hand us that space converted to "physical" px,
//! but every source multiplies by a *different* scale factor: the tray rect by
//! the clicked display's, each monitor's bounds by its own, and
//! `set_position(Physical…)` divides by the popover's *current* display's. On a
//! mixed-DPI setup (2× Retina laptop + 1× external) those spaces disagree, and
//! the popover landed on the wrong display. So every input is normalized back
//! to points first, and all math here — and the final `set_size`/`set_position`
//! — happens in points, independent of which display the window was last on.
//!
//! Pure (no Tauri types) so the display arrangements that caused the bug can be
//! unit-tested; `main.rs::position_popover` is the thin glue around it.
//!
//! **Windows** (#198) has no such mismatch: tao and tray-icon report monitors,
//! the cursor and the tray rect in one physical virtual-screen space, so the
//! glue feeds physical px straight in (via [`tray_in_global`] rather than
//! [`tray_to_logical`]). The tray there is in the taskbar, usually at the
//! bottom, so the popover opens *away* from the tray's edge ([`tray_edge`]) and
//! stays inside the display's work area, never over the taskbar. On macOS the
//! work area is the full display and the menu bar is at the top, so the result
//! is the same drop-down as before.

/// A rectangle in one global space: macOS points (origin top-left of the main
/// display, y growing downward — the space tao reports after its flip), or
/// Windows physical virtual-screen px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn contains(&self, (px, py): (f64, f64)) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    fn scaled(&self, by: f64) -> Rect {
        Rect { x: self.x / by, y: self.y / by, w: self.w / by, h: self.h / by }
    }
}

/// One display: its bounds, the part of them windows may use (minus the
/// Windows taskbar; the same as `bounds` on macOS), and its scale factor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    pub bounds: Rect,
    pub work_area: Rect,
    pub scale: f64,
}

/// Where the popover goes when there is no tray rect to anchor to: near the
/// tray's usual corner (top-right on macOS, bottom-right on Windows).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Corner {
    TopRight,
    BottomRight,
}

/// Which way the popover opens from the tray icon: away from the screen edge
/// the icon sits on. `Top` means the tray is at the top and the popover drops
/// down; `Bottom` means it opens upward; `Left`/`Right` open sideways.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

/// The popover's target frame, in points. `size` is `Some` only when the window
/// must shrink to fit the display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub display: usize,
    pub position: (f64, f64),
    pub size: Option<(f64, f64)>,
}

/// Small gap kept between the popover and the work area's edge: the display
/// corner with no tray rect to anchor to, and the taskbar's edge on Windows.
pub const EDGE_MARGIN: f64 = 8.0;

fn display_containing(displays: &[Display], p: (f64, f64)) -> Option<usize> {
    displays.iter().position(|d| d.bounds.contains(p))
}

/// Find the display of a tray rect that is already in the displays' own global
/// space (Windows): the one containing its center.
pub fn tray_in_global(tray: Rect, displays: &[Display]) -> Option<(Rect, usize)> {
    display_containing(displays, tray.center()).map(|i| (tray, i))
}

/// The edge of `work` (a display's work area) the tray icon is on. Outside the
/// work area means it's in the taskbar on that side; inside (the macOS menu
/// bar, or the Windows overflow flyout above the taskbar) the nearer of the
/// top and bottom halves wins.
pub fn tray_edge(tray: Rect, work: Rect) -> Edge {
    let (cx, cy) = tray.center();
    if cy >= work.y + work.h {
        Edge::Bottom
    } else if cy < work.y {
        Edge::Top
    } else if cx < work.x {
        Edge::Left
    } else if cx >= work.x + work.w {
        Edge::Right
    } else if cy < work.y + work.h / 2.0 {
        Edge::Top
    } else {
        Edge::Bottom
    }
}

/// Convert tray-icon's physical tray rect to points and find its display.
///
/// tray-icon multiplied the rect by the clicked display's scale, which we don't
/// know. `cursor` — the pointer in points, sampled when the rect was captured,
/// so it was over the icon — picks the display unambiguously. Without it, fall
/// back to the first display that contains the rect once divided by that
/// display's own scale; that alone can be ambiguous on mixed-DPI setups (a 1×
/// icon at px 3000 ÷ 2 = pt 1500 also lies inside a 2× laptop), hence the cursor.
pub fn tray_to_logical(
    tray_phys: Rect,
    cursor: Option<(f64, f64)>,
    displays: &[Display],
) -> Option<(Rect, usize)> {
    if let Some(i) = cursor.and_then(|c| display_containing(displays, c)) {
        return Some((tray_phys.scaled(displays[i].scale), i));
    }
    displays.iter().enumerate().find_map(|(i, d)| {
        let tray = tray_phys.scaled(d.scale);
        d.bounds.contains(tray.center()).then_some((tray, i))
    })
}

/// The display to use with no tray rect: the one under `fallback_point` (the
/// cursor), else the main display (the one at the origin), else the first.
fn fallback_display(displays: &[Display], fallback_point: Option<(f64, f64)>) -> usize {
    fallback_point
        .and_then(|p| display_containing(displays, p))
        .or_else(|| display_containing(displays, (0.0, 0.0)))
        .unwrap_or(0)
}

/// The display the popover will go on: the tray's, else the one under
/// `fallback_point`, else the main one. `None` only with no displays. Exposed
/// so the Windows glue can size the window for that display's scale first.
pub fn target_display(
    tray: Option<(Rect, usize)>,
    displays: &[Display],
    fallback_point: Option<(f64, f64)>,
) -> Option<usize> {
    if displays.is_empty() {
        return None;
    }
    Some(match tray {
        Some((_, i)) if i < displays.len() => i,
        _ => fallback_display(displays, fallback_point),
    })
}

/// Compute the popover frame. With a tray rect: centered on the icon and opened
/// away from the tray's edge ([`tray_edge`]) — dropped down from the macOS menu
/// bar, raised above the Windows taskbar — then shrunk to fit and clamped fully
/// into the tray display's work area. Without one: pinned to `corner` of the
/// fallback display's work area. `None` only if there are no displays at all.
pub fn place(
    tray: Option<(Rect, usize)>,
    win: (f64, f64),
    displays: &[Display],
    fallback_point: Option<(f64, f64)>,
    corner: Corner,
) -> Option<Placement> {
    let display = target_display(tray, displays, fallback_point)?;
    let a = displays[display].work_area;

    // Shrink to fit so the popover can never be cropped, whatever its saved size.
    let (w, h) = (win.0.min(a.w), win.1.min(a.h));
    let size = (w != win.0 || h != win.1).then_some((w, h));

    let (right, bottom) = (a.x + a.w, a.y + a.h);
    let (x, y) = match tray {
        Some((t, i)) if i == display => {
            let (cx, cy) = t.center();
            match tray_edge(t, a) {
                Edge::Top => (cx - w / 2.0, t.y + t.h),
                Edge::Bottom => (cx - w / 2.0, t.y.min(bottom) - h - EDGE_MARGIN),
                Edge::Left => ((t.x + t.w).max(a.x) + EDGE_MARGIN, cy - h / 2.0),
                Edge::Right => (t.x.min(right) - w - EDGE_MARGIN, cy - h / 2.0),
            }
        }
        _ => match corner {
            Corner::TopRight => (right - w - EDGE_MARGIN, a.y + EDGE_MARGIN),
            Corner::BottomRight => (right - w - EDGE_MARGIN, bottom - h - EDGE_MARGIN),
        },
    };
    // Floored at the top-left so a window as large as the work area pins to it.
    let x = x.clamp(a.x, (right - w).max(a.x));
    let y = y.clamp(a.y, (bottom - h).max(a.y));

    Some(Placement { display, position: (x, y), size })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    /// A macOS display: the work area is the whole display.
    fn display(x: f64, y: f64, w: f64, h: f64, scale: f64) -> Display {
        Display { bounds: rect(x, y, w, h), work_area: rect(x, y, w, h), scale }
    }

    /// 2× MacBook (main, at the origin) and a 1× 1920×1080 external.
    const LAPTOP: (f64, f64) = (1512.0, 982.0);
    fn laptop() -> Display {
        display(0.0, 0.0, LAPTOP.0, LAPTOP.1, 2.0)
    }

    /// A 24×24pt tray icon at `(x, y)` points, as tray-icon reports it: in
    /// physical px of its display's `scale`.
    fn tray_phys(x: f64, y: f64, scale: f64) -> Rect {
        rect(x * scale, y * scale, 24.0 * scale, 24.0 * scale)
    }

    const WIN: (f64, f64) = (680.0, 460.0);

    fn place_click(tray_pt: (f64, f64), scale: f64, displays: &[Display]) -> Placement {
        let cursor = (tray_pt.0 + 12.0, tray_pt.1 + 12.0);
        let tray = tray_to_logical(tray_phys(tray_pt.0, tray_pt.1, scale), Some(cursor), displays);
        place(tray, WIN, displays, Some(cursor), Corner::TopRight).unwrap()
    }

    fn assert_under_icon(p: Placement, tray_pt: (f64, f64), d: &Display) {
        assert_eq!(p.position.1, tray_pt.1 + 24.0, "drops down from the menu bar");
        assert_eq!(p.position.0, tray_pt.0 + 12.0 - WIN.0 / 2.0, "centered under the icon");
        assert!(d.bounds.contains(p.position));
    }

    #[test]
    fn external_right_1x_tray_while_main_is_2x() {
        let displays = [laptop(), display(1512.0, 0.0, 1920.0, 1080.0, 1.0)];
        let tray = (3000.0, 0.0);
        let p = place_click(tray, 1.0, &displays);
        assert_eq!(p.display, 1);
        assert_under_icon(p, tray, &displays[1]);
    }

    #[test]
    fn tray_on_2x_display_with_1x_main() {
        // Main is the 1× external; the 2× laptop sits to its right.
        let displays = [display(0.0, 0.0, 1920.0, 1080.0, 1.0), display(1920.0, 0.0, 1512.0, 982.0, 2.0)];
        let tray = (3000.0, 0.0);
        let p = place_click(tray, 2.0, &displays);
        assert_eq!(p.display, 1);
        assert_under_icon(p, tray, &displays[1]);
    }

    #[test]
    fn external_above_main() {
        let displays = [laptop(), display(-200.0, -1080.0, 1920.0, 1080.0, 1.0)];
        let tray = (1000.0, -1080.0);
        let p = place_click(tray, 1.0, &displays);
        assert_eq!(p.display, 1);
        assert_under_icon(p, tray, &displays[1]);
    }

    #[test]
    fn external_below_main_drops_down_not_up() {
        let displays = [laptop(), display(0.0, 982.0, 1920.0, 1080.0, 1.0)];
        let tray = (1400.0, 982.0);
        let p = place_click(tray, 1.0, &displays);
        assert_eq!(p.display, 1);
        assert_under_icon(p, tray, &displays[1]);
    }

    #[test]
    fn external_left_negative_x() {
        let displays = [laptop(), display(-1920.0, 0.0, 1920.0, 1080.0, 1.0)];
        let tray = (-500.0, 0.0);
        let p = place_click(tray, 1.0, &displays);
        assert_eq!(p.display, 1);
        assert_under_icon(p, tray, &displays[1]);
    }

    #[test]
    fn cursor_disambiguates_overlapping_scaled_rects() {
        // px 3000 on the 1× external ÷ 2 = pt 1500, which is inside the 2× laptop.
        let displays = [laptop(), display(1512.0, 0.0, 1920.0, 1080.0, 1.0)];
        let (tray, i) = tray_to_logical(tray_phys(3000.0, 0.0, 1.0), Some((3010.0, 10.0)), &displays).unwrap();
        assert_eq!(i, 1);
        assert_eq!(tray, rect(3000.0, 0.0, 24.0, 24.0));
    }

    #[test]
    fn without_cursor_falls_back_to_scale_division() {
        let displays = [laptop(), display(1512.0, 0.0, 1920.0, 1080.0, 1.0)];
        let (tray, i) = tray_to_logical(tray_phys(3300.0, 0.0, 1.0), None, &displays).unwrap();
        assert_eq!(i, 1);
        assert_eq!(tray.x, 3300.0);
    }

    #[test]
    fn larger_than_display_shrinks_to_fit() {
        let displays = [laptop()];
        let cursor = (1400.0, 10.0);
        let tray = tray_to_logical(tray_phys(1390.0, 0.0, 2.0), Some(cursor), &displays);
        let p = place(tray, (3000.0, 2000.0), &displays, Some(cursor), Corner::TopRight).unwrap();
        assert_eq!(p.size, Some(LAPTOP));
        assert_eq!(p.position, (0.0, 0.0));
    }

    #[test]
    fn far_right_tray_clamps_on_screen() {
        let displays = [laptop()];
        let p = place_click((1490.0, 0.0), 2.0, &displays);
        assert_eq!(p.size, None);
        assert_eq!(p.position, (LAPTOP.0 - WIN.0, 24.0));
    }

    #[test]
    fn no_tray_uses_cursor_display_top_right() {
        let displays = [laptop(), display(1512.0, 0.0, 1920.0, 1080.0, 1.0)];
        let p = place(None, WIN, &displays, Some((2000.0, 500.0)), Corner::TopRight).unwrap();
        assert_eq!(p.display, 1);
        assert_eq!(p.position, (1512.0 + 1920.0 - WIN.0 - EDGE_MARGIN, EDGE_MARGIN));
    }

    #[test]
    fn no_tray_no_cursor_uses_main_display() {
        let displays = [display(1512.0, 0.0, 1920.0, 1080.0, 1.0), laptop()];
        let p = place(None, WIN, &displays, None, Corner::TopRight).unwrap();
        assert_eq!(p.display, 1);
        assert_eq!(p.position, (LAPTOP.0 - WIN.0 - EDGE_MARGIN, EDGE_MARGIN));
    }

    #[test]
    fn no_displays_places_nothing() {
        assert_eq!(place(None, WIN, &[], None, Corner::TopRight), None);
    }

    // ── Windows: physical px, the tray in the taskbar (#198) ──────────────────

    /// A 1920×1080 monitor at `x` whose taskbar (48px at 100% scale) is on the
    /// bottom edge: the work area stops above it.
    fn win_bottom_taskbar(x: f64, scale: f64) -> Display {
        let (w, h, bar) = (1920.0 * scale, 1080.0 * scale, 48.0 * scale);
        Display { bounds: rect(x, 0.0, w, h), work_area: rect(x, 0.0, w, h - bar), scale }
    }

    /// A 24px-at-100% tray icon centered in the bottom taskbar of `d`, `from_right`
    /// px left of the monitor's right edge.
    fn win_tray(d: &Display, from_right: f64) -> Rect {
        let s = 24.0 * d.scale;
        let bar_top = d.work_area.y + d.work_area.h;
        let bar_h = d.bounds.h - d.work_area.h;
        rect(d.bounds.x + d.bounds.w - from_right, bar_top + (bar_h - s) / 2.0, s, s)
    }

    fn place_win(tray: Rect, win: (f64, f64), displays: &[Display]) -> Placement {
        let tray = tray_in_global(tray, displays);
        place(tray, win, displays, None, Corner::BottomRight).unwrap()
    }

    /// Asserts the popover sits fully inside `d`'s work area, i.e. never over
    /// the taskbar.
    fn assert_in_work_area(p: Placement, win: (f64, f64), d: &Display) {
        let a = d.work_area;
        let (w, h) = p.size.unwrap_or(win);
        assert!(p.position.0 >= a.x && p.position.0 + w <= a.x + a.w, "x out of work area: {p:?}");
        assert!(p.position.1 >= a.y && p.position.1 + h <= a.y + a.h, "y out of work area: {p:?}");
    }

    #[test]
    fn windows_bottom_taskbar_opens_above_the_tray() {
        let displays = [win_bottom_taskbar(0.0, 1.0)];
        // Far enough from the right edge that centering needs no clamp.
        let tray = win_tray(&displays[0], 700.0);
        let p = place_win(tray, WIN, &displays);
        assert_eq!(tray_edge(tray, displays[0].work_area), Edge::Bottom);
        // Its bottom edge sits EDGE_MARGIN above the taskbar, centered on the icon.
        assert_eq!(p.position.1, 1080.0 - 48.0 - WIN.1 - EDGE_MARGIN);
        assert_eq!(p.position.0, tray.x + tray.w / 2.0 - WIN.0 / 2.0);
        assert_in_work_area(p, WIN, &displays[0]);
    }

    #[test]
    fn windows_bottom_taskbar_at_150_percent() {
        let displays = [win_bottom_taskbar(0.0, 1.5)];
        let win = (WIN.0 * 1.5, WIN.1 * 1.5); // the glue sizes it for the display
        let tray = win_tray(&displays[0], 450.0);
        let p = place_win(tray, win, &displays);
        assert_eq!(p.position.1, displays[0].work_area.h - win.1 - EDGE_MARGIN);
        assert_in_work_area(p, win, &displays[0]);
    }

    #[test]
    fn windows_tray_near_the_corner_clamps_left_but_stays_above() {
        let displays = [win_bottom_taskbar(0.0, 1.0)];
        let p = place_win(win_tray(&displays[0], 40.0), WIN, &displays);
        assert_eq!(p.position.0, 1920.0 - WIN.0, "clamped to the right edge");
        assert_eq!(p.position.1, 1080.0 - 48.0 - WIN.1 - EDGE_MARGIN);
    }

    #[test]
    fn windows_mixed_dpi_tray_on_the_secondary_monitor() {
        // 150% laptop panel on the left, 100% external on the right.
        let displays = [win_bottom_taskbar(0.0, 1.5), win_bottom_taskbar(2880.0, 1.0)];
        let tray = win_tray(&displays[1], 300.0);
        let p = place_win(tray, WIN, &displays);
        assert_eq!(p.display, 1);
        assert_in_work_area(p, WIN, &displays[1]);
    }

    #[test]
    fn windows_side_taskbars_open_sideways() {
        // Taskbar on the left: 64px wide.
        let left = Display { bounds: rect(0.0, 0.0, 1920.0, 1080.0), work_area: rect(64.0, 0.0, 1856.0, 1080.0), scale: 1.0 };
        let tray = rect(20.0, 900.0, 24.0, 24.0);
        assert_eq!(tray_edge(tray, left.work_area), Edge::Left);
        let p = place_win(tray, WIN, &[left]);
        assert_eq!(p.position.0, 64.0 + EDGE_MARGIN);
        assert_in_work_area(p, WIN, &left);

        // Taskbar on the right.
        let right = Display { bounds: rect(0.0, 0.0, 1920.0, 1080.0), work_area: rect(0.0, 0.0, 1856.0, 1080.0), scale: 1.0 };
        let tray = rect(1876.0, 900.0, 24.0, 24.0);
        assert_eq!(tray_edge(tray, right.work_area), Edge::Right);
        let p = place_win(tray, WIN, &[right]);
        assert_eq!(p.position.0, 1856.0 - WIN.0 - EDGE_MARGIN);
        assert_in_work_area(p, WIN, &right);
    }

    #[test]
    fn windows_top_taskbar_drops_down() {
        let top = Display { bounds: rect(0.0, 0.0, 1920.0, 1080.0), work_area: rect(0.0, 48.0, 1920.0, 1032.0), scale: 1.0 };
        let tray = rect(1600.0, 12.0, 24.0, 24.0);
        assert_eq!(tray_edge(tray, top.work_area), Edge::Top);
        let p = place_win(tray, WIN, &[top]);
        assert_eq!(p.position.1, 48.0, "just below the taskbar");
    }

    #[test]
    fn windows_overflow_flyout_icon_opens_above_it() {
        // The `^` flyout sits inside the work area, just above the taskbar.
        let displays = [win_bottom_taskbar(0.0, 1.0)];
        let tray = rect(1500.0, 950.0, 24.0, 24.0);
        assert_eq!(tray_edge(tray, displays[0].work_area), Edge::Bottom);
        let p = place_win(tray, WIN, &displays);
        assert_eq!(p.position.1, 950.0 - WIN.1 - EDGE_MARGIN);
    }

    #[test]
    fn windows_no_tray_pins_bottom_right_of_the_work_area() {
        let displays = [win_bottom_taskbar(0.0, 1.0)];
        let p = place(None, WIN, &displays, Some((500.0, 500.0)), Corner::BottomRight).unwrap();
        assert_eq!(p.position, (1920.0 - WIN.0 - EDGE_MARGIN, 1032.0 - WIN.1 - EDGE_MARGIN));
    }

    #[test]
    fn windows_taller_than_the_work_area_shrinks_to_it() {
        let displays = [win_bottom_taskbar(0.0, 1.0)];
        let p = place_win(win_tray(&displays[0], 300.0), (680.0, 1500.0), &displays);
        assert_eq!(p.size, Some((680.0, 1032.0)));
        assert_eq!(p.position.1, 0.0);
    }
}
