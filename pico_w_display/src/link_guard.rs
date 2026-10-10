//! Which BLE links may send Commands (`CONTEXT.md`'s Claimed entry,
//! ADR-0005): only the Claimed phone's, encrypted with the stored Bond, or
//! the link whose Pairing just Claimed the device. Any other link is
//! disconnected before a Command reaches its Session. `bt.rs` maps
//! `trouble-host`'s connection events onto [`LinkEvent`] and acts on each
//! [`Verdict`].

/// What happened on one BLE link, as far as the guard cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkEvent {
    /// The link is encrypted with the stored Bond: the Claimed phone,
    /// reconnecting.
    ResumedBond,
    /// A Pairing finished on the link; `claimed` if it just Claimed the
    /// device (it was Unclaimed and produced a Bond).
    Paired { claimed: bool },
    /// A `command` write arrived.
    Command,
}

/// What `bt.rs` should do about a [`LinkEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing to do.
    Continue,
    /// Hand the Command to the Session.
    Forward,
    /// Drop the link; nothing it sent reaches the Session.
    Disconnect,
}

/// One BLE link's standing. A fresh link can't send Commands until it has
/// shown it's the Claimed phone's.
#[derive(Debug, Default)]
pub struct LinkGuard {
    owner: bool,
}

impl LinkGuard {
    pub const fn new() -> Self {
        Self { owner: false }
    }

    pub fn on(&mut self, event: LinkEvent) -> Verdict {
        match event {
            LinkEvent::ResumedBond | LinkEvent::Paired { claimed: true } => {
                self.owner = true;
                Verdict::Continue
            }
            LinkEvent::Paired { claimed: false } => {
                self.owner = false;
                Verdict::Disconnect
            }
            LinkEvent::Command if self.owner => Verdict::Forward,
            LinkEvent::Command => Verdict::Disconnect,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(events: &[LinkEvent]) -> std::vec::Vec<Verdict> {
        let mut guard = LinkGuard::new();
        events.iter().map(|&e| guard.on(e)).collect()
    }

    #[test]
    fn claimed_phone_reconnecting_with_its_bond_may_send_commands() {
        assert_eq!(
            run(&[LinkEvent::ResumedBond, LinkEvent::Command, LinkEvent::Command]),
            [Verdict::Continue, Verdict::Forward, Verdict::Forward]
        );
    }

    #[test]
    fn pairing_that_claims_the_device_may_send_commands() {
        assert_eq!(
            run(&[LinkEvent::Paired { claimed: true }, LinkEvent::Command]),
            [Verdict::Continue, Verdict::Forward]
        );
    }

    #[test]
    fn pairing_that_does_not_claim_is_disconnected() {
        // A second phone on a Claimed device, or a phone that wouldn't bond.
        assert_eq!(
            run(&[LinkEvent::Paired { claimed: false }]),
            [Verdict::Disconnect]
        );
    }

    #[test]
    fn transient_link_never_gets_a_command_through() {
        assert_eq!(
            run(&[LinkEvent::Paired { claimed: false }, LinkEvent::Command]),
            [Verdict::Disconnect, Verdict::Disconnect]
        );
    }

    #[test]
    fn command_before_the_link_is_the_owners_is_disconnected() {
        assert_eq!(run(&[LinkEvent::Command]), [Verdict::Disconnect]);
    }

    #[test]
    fn owner_re_pairing_without_a_bond_loses_its_standing() {
        // e.g. the Claimed phone forgot its keys and paired afresh: only a
        // Factory Reset lets it Claim the device again.
        assert_eq!(
            run(&[
                LinkEvent::ResumedBond,
                LinkEvent::Paired { claimed: false },
                LinkEvent::Command,
            ]),
            [Verdict::Continue, Verdict::Disconnect, Verdict::Disconnect]
        );
    }
}
