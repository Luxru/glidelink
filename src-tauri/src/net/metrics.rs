//! Application RTT estimation for reporting and conservative pointer pacing.

/// RFC 6298 RTT smoother. `rttvar` is what the UI calls jitter.
#[derive(Clone, Debug)]
pub struct RttEstimator {
    srtt_ms: Option<f64>,
    rttvar_ms: f64,
}

impl RttEstimator {
    pub fn new() -> Self {
        Self {
            srtt_ms: None,
            rttvar_ms: 0.0,
        }
    }

    pub fn observe(&mut self, rtt_ms: f64) {
        let rtt = rtt_ms.max(0.0);
        match self.srtt_ms {
            None => {
                self.srtt_ms = Some(rtt);
                self.rttvar_ms = rtt / 2.0;
            }
            Some(srtt) => {
                // RFC 6298 section 2.3: update RTTVAR with the old SRTT,
                // then update SRTT.
                self.rttvar_ms = 0.75 * self.rttvar_ms + 0.25 * (srtt - rtt).abs();
                self.srtt_ms = Some(0.875 * srtt + 0.125 * rtt);
            }
        }
    }

    pub fn srtt_ms(&self) -> Option<f64> {
        self.srtt_ms
    }

    pub fn jitter_ms(&self) -> f64 {
        self.rttvar_ms
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

        rtt.observe(120.0);
        close(rtt.srtt_ms().unwrap(), 102.5);
        close(rtt.jitter_ms(), 42.5);

        rtt.observe(80.0);
        close(rtt.jitter_ms(), 0.75 * 42.5 + 0.25 * 22.5);
        close(rtt.srtt_ms().unwrap(), 0.875 * 102.5 + 0.125 * 80.0);
    }
}
