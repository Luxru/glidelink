//! Link-quality estimators used only for reporting, not for pacing.

/// RFC 6298 RTT smoother. `rttvar` is what the UI calls jitter.
#[derive(Clone, Debug)]
pub struct RttEstimator {
    srtt_ms: Option<f64>,
    rttvar_ms: f64,
    min_rtt_ms: f64,
    samples: u64,
}

impl RttEstimator {
    pub fn new() -> Self {
        Self {
            srtt_ms: None,
            rttvar_ms: 0.0,
            min_rtt_ms: f64::INFINITY,
            samples: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn observe(&mut self, rtt_ms: f64) {
        let rtt = rtt_ms.max(0.0);
        match self.srtt_ms {
            None => {
                self.srtt_ms = Some(rtt);
                self.rttvar_ms = rtt / 2.0;
                self.min_rtt_ms = rtt;
            }
            Some(srtt) => {
                // RFC 6298 section 2.3: update RTTVAR with the old SRTT,
                // then update SRTT.
                self.rttvar_ms = 0.75 * self.rttvar_ms + 0.25 * (srtt - rtt).abs();
                self.srtt_ms = Some(0.875 * srtt + 0.125 * rtt);
                self.min_rtt_ms = self.min_rtt_ms.min(rtt);
            }
        }
        self.samples = self.samples.saturating_add(1);
    }

    pub fn srtt_ms(&self) -> Option<f64> {
        self.srtt_ms
    }

    pub fn jitter_ms(&self) -> f64 {
        self.rttvar_ms
    }

    pub fn min_rtt_ms(&self) -> Option<f64> {
        if self.samples == 0 {
            None
        } else {
            Some(self.min_rtt_ms)
        }
    }
}

/// RFC 3550-style receiver counts over the controller-to-agent sequence
/// space. Only call `observe` after `decrypt_packet_with_seq` accepted the
/// packet; duplicates are then already excluded by the replay window.
#[derive(Clone, Debug)]
pub struct LossTracker {
    base: u64,
    highest: u64,
    received: u64,
}

impl LossTracker {
    pub fn new(base: u64) -> Self {
        Self {
            base,
            highest: base,
            received: 0,
        }
    }

    pub fn observe(&mut self, seq: u64) {
        if seq <= self.base {
            return;
        }
        self.highest = self.highest.max(seq);
        self.received = self.received.saturating_add(1);
    }

    pub fn expected(&self) -> u64 {
        self.highest.saturating_sub(self.base)
    }

    pub fn received(&self) -> u64 {
        self.received
    }

    pub fn loss_percent(&self) -> Option<f64> {
        let expected = self.expected();
        if expected == 0 {
            return None;
        }
        let lost = expected.saturating_sub(self.received) as f64;
        Some((lost / expected as f64 * 100.0).clamp(0.0, 100.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn rtt_estimator_follows_rfc_6298() {
        let mut rtt = RttEstimator::new();
        rtt.observe(100.0);
        close(rtt.srtt_ms().unwrap(), 100.0);
        close(rtt.jitter_ms(), 50.0);
        close(rtt.min_rtt_ms().unwrap(), 100.0);

        rtt.observe(120.0);
        close(rtt.srtt_ms().unwrap(), 102.5);
        close(rtt.jitter_ms(), 42.5);
        close(rtt.min_rtt_ms().unwrap(), 100.0);

        rtt.observe(80.0);
        close(rtt.jitter_ms(), 0.75 * 42.5 + 0.25 * 22.5);
        close(rtt.srtt_ms().unwrap(), 0.875 * 102.5 + 0.125 * 80.0);
        close(rtt.min_rtt_ms().unwrap(), 80.0);
    }

    #[test]
    fn loss_counts_gaps_and_heals_on_late_arrival() {
        let mut loss = LossTracker::new(10);
        assert_eq!(loss.loss_percent(), None);
        loss.observe(11);
        loss.observe(13);
        assert_eq!(loss.expected(), 3);
        assert_eq!(loss.received(), 2);
        close(loss.loss_percent().unwrap(), 100.0 / 3.0);

        loss.observe(12);
        assert_eq!(loss.received(), 3);
        close(loss.loss_percent().unwrap(), 0.0);
        loss.observe(10);
        assert_eq!(loss.received(), 3);
    }

    #[test]
    fn loss_saturates_at_zero_when_reordering_exceeds_expected() {
        let mut loss = LossTracker::new(5);
        loss.observe(6);
        loss.observe(7);
        loss.observe(8);
        close(loss.loss_percent().unwrap(), 0.0);
        loss.observe(9);
        close(loss.loss_percent().unwrap(), 0.0);
    }
}
