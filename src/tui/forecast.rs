//! Forecast-view state for the TUI.
//!
//! [`ForecastView`] holds the horizon (seeded from config) and offers a small
//! adjuster. The entries themselves are computed on demand from the app's
//! cached tasks via [`crate::core::forecast::build_entries`], honouring the
//! app's active filter tokens/flags.

/// Live state for the forecast view: just the horizon in days.
pub struct ForecastView {
    /// Forecast window in days; seeded from `config.forecast_horizon_days`.
    horizon: u32,
}

impl ForecastView {
    /// Builds the view with the given default horizon.
    pub fn new(horizon: u32) -> Self {
        Self { horizon }
    }

    /// The current horizon in days.
    pub fn horizon(&self) -> u32 {
        self.horizon
    }

    /// Widens the horizon by `days` (capped to keep projection bounded).
    pub fn widen(&mut self, days: u32) {
        self.horizon = (self.horizon + days).min(3650);
    }

    /// Narrows the horizon by `days` (never below 1).
    pub fn narrow(&mut self, days: u32) {
        self.horizon = self.horizon.saturating_sub(days).max(1);
    }
}

/// The section a forecast entry falls into, by day-delta from today. Mirrors
/// the `next forecast` CLI grouping so both surfaces read the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Overdue,
    Today,
    ThisWeek,
    ThisMonth,
    Later,
}

impl Section {
    /// Classifies an entry by how many days away its date is.
    pub fn classify(days_from_today: i64) -> Self {
        match days_from_today {
            d if d < 0 => Section::Overdue,
            0 => Section::Today,
            1..=7 => Section::ThisWeek,
            8..=30 => Section::ThisMonth,
            _ => Section::Later,
        }
    }

    /// The heading label; `Later` carries the horizon so it reads "Next N days".
    pub fn label(self, horizon: u32) -> String {
        match self {
            Section::Overdue => "Overdue".to_owned(),
            Section::Today => "Today".to_owned(),
            Section::ThisWeek => "This week".to_owned(),
            Section::ThisMonth => "This month".to_owned(),
            Section::Later => format!("Next {horizon} days"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizon_adjusts_within_bounds() {
        let mut v = ForecastView::new(30);
        v.widen(30);
        assert_eq!(v.horizon(), 60);
        v.narrow(100);
        assert_eq!(v.horizon(), 1, "never below 1");
    }

    #[test]
    fn classify_matches_cli_bands() {
        assert_eq!(Section::classify(-1), Section::Overdue);
        assert_eq!(Section::classify(0), Section::Today);
        assert_eq!(Section::classify(7), Section::ThisWeek);
        assert_eq!(Section::classify(8), Section::ThisMonth);
        assert_eq!(Section::classify(30), Section::ThisMonth);
        assert_eq!(Section::classify(31), Section::Later);
    }
}
