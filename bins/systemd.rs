//! Best-effort systemd service-manager notifications shared by both binaries.

use std::time::Duration;

use sd_notify::NotifyState;
use tokio::{sync::watch, task::JoinHandle};

/// A no-op outside a systemd `Type=notify` unit or when explicitly disabled.
#[derive(Clone, Copy)]
pub struct Notifier {
    enabled: bool,
    watchdog_interval: Option<Duration>,
}

impl Notifier {
    pub fn detect(enabled: bool) -> Self {
        let managed = enabled && std::env::var_os("NOTIFY_SOCKET").is_some();
        Self {
            enabled: managed,
            watchdog_interval: managed
                .then(sd_notify::watchdog_enabled)
                .flatten()
                .map(watchdog_cadence),
        }
    }

    pub fn ready(self, status: &'static str) {
        self.send(&[NotifyState::Ready, NotifyState::Status(status)]);
    }

    pub fn stopping(self, status: &'static str) {
        self.send(&[NotifyState::Stopping, NotifyState::Status(status)]);
    }

    pub fn spawn_watchdog(self, mut shutdown: watch::Receiver<bool>) -> Option<JoinHandle<()>> {
        let cadence = self.watchdog_interval?;
        Some(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(cadence);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // `interval` ticks immediately; readiness already proves the process is alive.
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = ticker.tick() => self.send(&[NotifyState::Watchdog]),
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { break; }
                    }
                }
            }
        }))
    }

    fn send(self, states: &[NotifyState<'_>]) {
        if self.enabled {
            if let Err(error) = sd_notify::notify(states) {
                tracing::warn!(%error, "systemd notification failed");
            }
        }
    }
}

fn watchdog_cadence(period: Duration) -> Duration {
    period.div_f64(2.0).max(Duration::from_millis(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_uses_half_period_with_a_safe_floor() {
        assert_eq!(
            watchdog_cadence(Duration::from_secs(20)),
            Duration::from_secs(10)
        );
        assert_eq!(
            watchdog_cadence(Duration::from_micros(1)),
            Duration::from_millis(1)
        );
    }
}
