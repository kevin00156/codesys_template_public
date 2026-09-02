//! LRW working-counter bookkeeping for the cyclic path — plain data, no bus
//! types, so it unit-tests on every platform even though only the Linux
//! backend drives it.
//!
//! ethercrab's `tx_rx`/`tx_rx_dc` return the working counter but never
//! validate it. A subdevice that stops processing the LRW (bad cable, tail
//! node physically gone) leaves the AL-status bitmap still ORing to OP, so
//! without this check its inputs would go stale silently. The expected value
//! is the counter of the first cycle in which every subdevice reported OP;
//! every later cycle is compared against it. Any difference — lower *or*
//! higher (a hot-plugged node changes the PDI layout we validated at start)
//! — is a failure.

/// Outcome of one working-counter comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WkcVerdict {
    /// The counter matched the learned value (or nothing is learned yet).
    pub ok: bool,
    /// `ok` flipped since the previous cycle — log on these only, never per
    /// cycle (RT path).
    pub edge: bool,
}

#[derive(Debug)]
pub(crate) struct WkcMonitor {
    /// `None` until the group reached OP: nothing to compare against, every
    /// cycle passes.
    expected: Option<u16>,
    /// Previous cycle's verdict, for once-per-edge logging.
    was_ok: bool,
}

impl Default for WkcMonitor {
    fn default() -> WkcMonitor {
        WkcMonitor {
            expected: None,
            was_ok: true,
        }
    }
}

impl WkcMonitor {
    /// Record `wkc` as the expected value the first time the whole group is
    /// in OP; later calls are no-ops, so the wait-for-OP loop can call this
    /// every cycle without caring which one succeeds.
    pub(crate) fn learn(&mut self, all_op: bool, wkc: u16) {
        if all_op && self.expected.is_none() {
            self.expected = Some(wkc);
        }
    }

    /// The learned reference, `None` before the group reached OP.
    pub(crate) fn expected(&self) -> Option<u16> {
        self.expected
    }

    /// Compare one cycle's counter. Allocation- and formatting-free.
    pub(crate) fn check(&mut self, wkc: u16) -> WkcVerdict {
        let ok = self.expected.map_or(true, |e| e == wkc);
        let edge = ok != self.was_ok;
        self.was_ok = ok;
        WkcVerdict { ok, edge }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: WkcVerdict = WkcVerdict { ok: true, edge: false };

    #[test]
    fn passes_everything_before_learning() {
        let mut m = WkcMonitor::default();
        assert_eq!(m.check(0), OK);
        assert_eq!(m.check(7), OK);
        assert_eq!(m.expected(), None);
    }

    #[test]
    fn learns_only_when_all_op_and_only_once() {
        let mut m = WkcMonitor::default();
        m.learn(false, 1); // still transitioning: partial counter, ignore
        assert_eq!(m.expected(), None);
        m.learn(true, 3);
        assert_eq!(m.expected(), Some(3));
        m.learn(true, 6); // later cycles never overwrite the reference
        assert_eq!(m.expected(), Some(3));
    }

    #[test]
    fn mismatch_and_recovery_each_report_one_edge() {
        let mut m = WkcMonitor::default();
        m.learn(true, 6);
        assert_eq!(m.check(6), OK);
        // tail node gone: first bad cycle is the edge, the rest are not
        assert_eq!(m.check(3), WkcVerdict { ok: false, edge: true });
        assert_eq!(m.check(3), WkcVerdict { ok: false, edge: false });
        assert_eq!(m.check(0), WkcVerdict { ok: false, edge: false });
        // plugged back in
        assert_eq!(m.check(6), WkcVerdict { ok: true, edge: true });
        assert_eq!(m.check(6), OK);
    }

    #[test]
    fn higher_than_expected_is_also_a_mismatch() {
        let mut m = WkcMonitor::default();
        m.learn(true, 3);
        assert!(!m.check(6).ok);
    }
}
