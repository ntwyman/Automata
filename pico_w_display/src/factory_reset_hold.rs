//! Button A's Factory Reset hold: what a press shows, and whether it's
//! triggered, given when it went down and (if it has) came back up. The
//! on-device task that watches the button and acts on it lives in
//! `factory_reset.rs`.

/// How long Button A must be held to trigger a Factory Reset.
pub const HOLD_MS: u64 = 5_000;

/// Where a press of Button A stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// Nothing to show: not held, released early, or held too briefly to
    /// have lit a column — so a short press draws nothing at all.
    Idle,
    /// Still held: `filled` of the progress bar's columns are lit.
    Filling { filled: usize },
    /// Held for the full [`HOLD_MS`]: Factory Reset.
    Complete,
}

/// The state at `now_ms` of a press that went down at `pressed_ms` and, if
/// `released_ms` is `Some`, came up then. The bar is `width` columns and
/// fills one column per `HOLD_MS / width`, so it's full exactly as the hold
/// completes. A press held for the full [`HOLD_MS`] is [`Hold::Complete`]
/// whenever it's released after that; one released before it is
/// [`Hold::Idle`] from the release on.
pub fn hold_at(pressed_ms: u64, released_ms: Option<u64>, now_ms: u64, width: usize) -> Hold {
    let released = released_ms.filter(|&at| at <= now_ms);
    let held = released.unwrap_or(now_ms).saturating_sub(pressed_ms);
    if held >= HOLD_MS {
        return Hold::Complete;
    }
    if released.is_some() {
        return Hold::Idle;
    }
    match (held as usize * width) / HOLD_MS as usize {
        0 => Hold::Idle,
        filled => Hold::Filling { filled },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 17;

    #[test]
    fn full_hold_triggers() {
        assert_eq!(hold_at(1_000, None, 1_000 + HOLD_MS, W), Hold::Complete);
        assert_eq!(hold_at(1_000, None, 1_000 + HOLD_MS + 50, W), Hold::Complete);
    }

    #[test]
    fn released_after_a_full_hold_still_triggers() {
        // The task may only notice the release after the hold completed.
        let pressed = 1_000;
        let released = pressed + HOLD_MS + 10;
        assert_eq!(hold_at(pressed, Some(released), released + 5, W), Hold::Complete);
    }

    #[test]
    fn bar_fills_while_held() {
        assert_eq!(hold_at(0, None, HOLD_MS / 2, W), Hold::Filling { filled: 8 });
        assert_eq!(hold_at(0, None, HOLD_MS - 1, W), Hold::Filling { filled: 16 });
    }

    #[test]
    fn bar_never_shrinks_while_held() {
        let mut last = 0;
        for now in (0..HOLD_MS).step_by(7) {
            if let Hold::Filling { filled } = hold_at(0, None, now, W) {
                assert!(filled >= last && filled < W, "{filled} at {now}ms");
                last = filled;
            }
        }
        assert_eq!(last, W - 1);
    }

    #[test]
    fn early_release_cancels() {
        let pressed = 2_000;
        let released = pressed + 3_000;
        assert!(matches!(
            hold_at(pressed, None, released - 1, W),
            Hold::Filling { .. }
        ));
        assert_eq!(hold_at(pressed, Some(released), released, W), Hold::Idle);
        // And stays cancelled, however long after.
        assert_eq!(hold_at(pressed, Some(released), pressed + 2 * HOLD_MS, W), Hold::Idle);
    }

    #[test]
    fn short_press_is_a_no_op() {
        // A typical tap: nothing drawn while down, nothing after.
        let pressed = 500;
        for now in pressed..pressed + 200 {
            assert_eq!(hold_at(pressed, None, now, W), Hold::Idle, "at {now}ms");
        }
        assert_eq!(hold_at(pressed, Some(pressed + 200), pressed + 200, W), Hold::Idle);
    }

    #[test]
    fn first_column_lights_after_one_columns_worth() {
        let column_ms = HOLD_MS / W as u64;
        assert_eq!(hold_at(0, None, column_ms - 1, W), Hold::Idle);
        assert_eq!(hold_at(0, None, column_ms + 1, W), Hold::Filling { filled: 1 });
    }

    #[test]
    fn clock_before_press_is_idle() {
        // `now` can't really precede the press, but mustn't underflow.
        assert_eq!(hold_at(1_000, None, 0, W), Hold::Idle);
    }
}
