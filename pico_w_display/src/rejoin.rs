//! When the device's Rejoin to its Saved Network should next try, after a
//! drop or a failed attempt. The on-device supervisor that acts on it lives
//! in `wifi.rs`.

/// How long to wait before the next Rejoin attempt after `failures`
/// consecutive failed ones: 10s, 30s, 1m, then 5m from then on. The first
/// step (`failures == 0`) doubles as the grace period after a drop, in case
/// the chip re-associates on its own.
pub fn retry_delay(failures: u32) -> u64 {
    const STEPS: [u64; 4] = [10, 30, 60, 5 * 60];
    STEPS[(failures as usize).min(STEPS.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backs_off_to_5_minutes() {
        let delays: std::vec::Vec<u64> = (0..=5).map(retry_delay).collect();
        assert_eq!(delays, [10, 30, 60, 300, 300, 300]);
        assert_eq!(retry_delay(u32::MAX), 300);
    }
}
