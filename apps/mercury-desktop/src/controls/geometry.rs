//! Where popups for page controls go in the window.
//!
//! Pointer input goes the other way: a position relative to the `web-view-input` area, in
//! logical pixels, is multiplied by the scale factor and sent as a device point
//! (`talaria-servo/src/web_view.rs`). Servo 0.8.0 reports a control's rectangle in CSS pixels
//! relative to the page's viewport (see `talaria_core::ControlRect`), so the way back is a
//! multiplication by the page zoom and the offset of that same area in the window; the display's
//! scale factor does not enter.

use talaria_core::ControlRect;

/// A rectangle in the window's logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl Rect {
    fn bottom(&self) -> f32 {
        self.y + self.height
    }

    fn right(&self) -> f32 {
        self.x + self.width
    }
}

/// The element's rectangle in the window. `page` is the web-view area in the window.
pub(crate) fn element_in_window(rect: ControlRect, page: Rect, zoom_percent: u16) -> Rect {
    let zoom = f32::from(zoom_percent) / 100.0;
    Rect {
        x: page.x + rect.x as f32 * zoom,
        y: page.y + rect.y as f32 * zoom,
        width: rect.width as f32 * zoom,
        height: rect.height as f32 * zoom,
    }
}

/// A popup of `width` × `height` attached to `anchor`: below it if it fits, otherwise on the
/// side with more room, made shorter if neither side has room; left-aligned with the anchor
/// and moved left as far as needed to stay inside `bounds`.
pub(crate) fn place_beside(anchor: Rect, width: f32, height: f32, bounds: Rect) -> Rect {
    let width = width.min(bounds.width).max(0.0);
    let room_below = (bounds.bottom() - anchor.bottom()).max(0.0);
    let room_above = (anchor.y - bounds.y).max(0.0);
    let (y, height) = if height <= room_below || room_below >= room_above {
        let top = anchor.bottom().clamp(bounds.y, bounds.bottom());
        (top, height.min(bounds.bottom() - top))
    } else {
        let bottom = anchor.y.clamp(bounds.y, bounds.bottom());
        let height = height.min(bottom - bounds.y);
        (bottom - height, height)
    };
    Rect {
        x: clamp_start(anchor.x, width, bounds.x, bounds.right()),
        y,
        width,
        height,
    }
}

/// A popup whose top-left corner is at the point, flipped to the left of or above the point
/// where it would leave `bounds`, then clamped into them.
pub(crate) fn place_at_point(x: f32, y: f32, width: f32, height: f32, bounds: Rect) -> Rect {
    let width = width.min(bounds.width).max(0.0);
    let height = height.min(bounds.height).max(0.0);
    let x = if x + width > bounds.right() {
        x - width
    } else {
        x
    };
    let y = if y + height > bounds.bottom() {
        y - height
    } else {
        y
    };
    Rect {
        x: clamp_start(x, width, bounds.x, bounds.right()),
        y: clamp_start(y, height, bounds.y, bounds.bottom()),
        width,
        height,
    }
}

/// The scroll offset (distance scrolled from the top) that shows the row whole, moving as
/// little as possible from `offset`.
pub(crate) fn reveal(row_top: f32, row_height: f32, viewport_height: f32, offset: f32) -> f32 {
    if row_top < offset {
        row_top
    } else if row_top + row_height > offset + viewport_height {
        (row_top + row_height - viewport_height).max(0.0)
    } else {
        offset
    }
}

fn clamp_start(start: f32, length: f32, min: f32, max: f32) -> f32 {
    start.min(max - length).max(min)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window's web-view area below a 48 px tab strip and a 60 px toolbar.
    const PAGE: Rect = Rect {
        x: 0.0,
        y: 108.0,
        width: 800.0,
        height: 492.0,
    };

    const ELEMENT: ControlRect = ControlRect {
        x: 40,
        y: 30,
        width: 150,
        height: 20,
    };

    #[test]
    fn at_normal_zoom_the_element_is_offset_by_the_toolbar() {
        assert_eq!(
            element_in_window(ELEMENT, PAGE, 100),
            Rect {
                x: 40.0,
                y: 138.0,
                width: 150.0,
                height: 20.0,
            }
        );
    }

    #[test]
    fn page_zoom_scales_the_element_but_not_the_offset() {
        assert_eq!(
            element_in_window(ELEMENT, PAGE, 200),
            Rect {
                x: 80.0,
                y: 168.0,
                width: 300.0,
                height: 40.0,
            }
        );
        assert_eq!(
            element_in_window(ELEMENT, PAGE, 50),
            Rect {
                x: 20.0,
                y: 123.0,
                width: 75.0,
                height: 10.0,
            }
        );
    }

    /// The pointer path in reverse: `talaria-servo` sends a pointer at logical position `p` in
    /// the page area as the device point `p × scale` (`web_view.rs`, `device_point`), and Servo
    /// turns device points into CSS pixels by dividing by `scale × zoom` (its
    /// `device_pixels_per_css_pixel`). A control rectangle at that CSS position must come back
    /// to `p` at every scale factor and zoom.
    fn css_point_under_pointer(
        logical_x: f32,
        logical_y: f32,
        scale: f32,
        zoom_percent: u16,
    ) -> (i32, i32) {
        let zoom = f32::from(zoom_percent) / 100.0;
        let device = ((logical_x - PAGE.x) * scale, (logical_y - PAGE.y) * scale);
        (
            (device.0 / (scale * zoom)).round() as i32,
            (device.1 / (scale * zoom)).round() as i32,
        )
    }

    #[test]
    fn a_control_lands_where_the_pointer_that_hit_it_was_at_every_scale_and_zoom() {
        for scale in [1.0_f32, 2.0] {
            for zoom in [50_u16, 100, 200] {
                let (x, y) = css_point_under_pointer(300.0, 408.0, scale, zoom);
                let rect = ControlRect {
                    x,
                    y,
                    width: 0,
                    height: 0,
                };

                let placed = element_in_window(rect, PAGE, zoom);

                assert_eq!(
                    (placed.x, placed.y),
                    (300.0, 408.0),
                    "scale {scale}, zoom {zoom}"
                );
            }
        }
    }

    /// Measured on a 2× display at 100 %: the text field of `tests/pages/controls.html`, which
    /// the screenshot shows at logical (218, 396)–(355, 413) under a 108 px toolbar.
    #[test]
    fn servos_rectangle_for_a_real_text_field_matches_the_screenshot() {
        let measured = ControlRect {
            x: 218,
            y: 288,
            width: 137,
            height: 17,
        };

        assert_eq!(
            element_in_window(measured, PAGE, 100),
            Rect {
                x: 218.0,
                y: 396.0,
                width: 137.0,
                height: 17.0,
            }
        );
    }

    #[test]
    fn a_page_offset_from_the_left_moves_the_element_too() {
        let page = Rect { x: 12.0, ..PAGE };

        assert_eq!(element_in_window(ELEMENT, page, 100).x, 52.0);
    }

    #[test]
    fn a_popup_that_fits_goes_below_the_element() {
        let anchor = element_in_window(ELEMENT, PAGE, 100);

        assert_eq!(
            place_beside(anchor, 200.0, 120.0, PAGE),
            Rect {
                x: 40.0,
                y: 158.0,
                width: 200.0,
                height: 120.0,
            }
        );
    }

    #[test]
    fn a_popup_without_room_below_flips_above() {
        let anchor = Rect {
            x: 40.0,
            y: 560.0,
            width: 150.0,
            height: 20.0,
        };

        assert_eq!(
            place_beside(anchor, 200.0, 120.0, PAGE),
            Rect {
                x: 40.0,
                y: 440.0,
                width: 200.0,
                height: 120.0,
            }
        );
    }

    #[test]
    fn a_popup_too_tall_for_either_side_shrinks_on_the_roomier_side() {
        let near_bottom = Rect {
            x: 40.0,
            y: 400.0,
            width: 150.0,
            height: 20.0,
        };
        let near_top = Rect {
            y: 200.0,
            ..near_bottom
        };

        assert_eq!(
            place_beside(near_bottom, 200.0, 1000.0, PAGE),
            Rect {
                x: 40.0,
                y: 108.0,
                width: 200.0,
                height: 292.0,
            }
        );
        assert_eq!(
            place_beside(near_top, 200.0, 1000.0, PAGE),
            Rect {
                x: 40.0,
                y: 220.0,
                width: 200.0,
                height: 380.0,
            }
        );
    }

    #[test]
    fn a_popup_is_clamped_to_the_right_edge_and_to_the_width() {
        let anchor = Rect {
            x: 750.0,
            y: 150.0,
            width: 40.0,
            height: 20.0,
        };

        assert_eq!(place_beside(anchor, 200.0, 100.0, PAGE).x, 600.0);
        assert_eq!(place_beside(anchor, 2000.0, 100.0, PAGE).width, 800.0);
        assert_eq!(place_beside(anchor, 2000.0, 100.0, PAGE).x, 0.0);
    }

    #[test]
    fn an_element_scrolled_partly_out_of_view_keeps_the_popup_inside() {
        let above_the_page = Rect {
            x: 40.0,
            y: 60.0,
            width: 150.0,
            height: 20.0,
        };

        let popup = place_beside(above_the_page, 200.0, 100.0, PAGE);

        assert_eq!(popup.y, 108.0);
        assert_eq!(popup.height, 100.0);
    }

    #[test]
    fn an_element_scrolled_below_the_page_opens_upwards_inside_it() {
        let below_the_page = Rect {
            x: 40.0,
            y: 700.0,
            width: 150.0,
            height: 20.0,
        };

        assert_eq!(
            place_beside(below_the_page, 200.0, 100.0, PAGE),
            Rect {
                x: 40.0,
                y: 500.0,
                width: 200.0,
                height: 100.0,
            }
        );
    }

    #[test]
    fn a_menu_opens_at_the_point_and_flips_at_the_edges() {
        assert_eq!(
            place_at_point(100.0, 200.0, 220.0, 300.0, PAGE),
            Rect {
                x: 100.0,
                y: 200.0,
                width: 220.0,
                height: 300.0,
            }
        );
        let flipped = place_at_point(700.0, 500.0, 220.0, 300.0, PAGE);
        assert_eq!((flipped.x, flipped.y), (480.0, 200.0));
    }

    #[test]
    fn a_menu_that_cannot_flip_is_clamped() {
        let menu = place_at_point(100.0, 300.0, 220.0, 450.0, PAGE);

        assert_eq!((menu.y, menu.height), (108.0, 450.0));
        let taller = place_at_point(100.0, 300.0, 220.0, 900.0, PAGE);
        assert_eq!((taller.y, taller.height), (108.0, 492.0));
    }

    #[test]
    fn revealing_a_row_scrolls_as_little_as_possible() {
        assert_eq!(reveal(48.0, 24.0, 120.0, 0.0), 0.0);
        assert_eq!(reveal(120.0, 24.0, 120.0, 0.0), 24.0);
        assert_eq!(reveal(24.0, 24.0, 120.0, 48.0), 24.0);
    }
}
