/// Weights used in the urgency scoring formula.
/// All values are additive contributions to the final score.
#[derive(Debug, Clone)]
pub struct ScoringConfig {
    // Due-date factor
    pub due_overdue_base: f64,
    pub due_overdue_per_day: f64,
    pub due_week_base: f64,
    pub due_week_per_day: f64,
    pub due_month_base: f64,
    pub due_month_per_day: f64,

    // Priority factor
    pub priority_low: f64,
    pub priority_medium: f64,
    pub priority_high: f64,

    // Project-priority offset
    pub project_low: f64,
    pub project_medium: f64,
    pub project_high: f64,

    // Age factor (per day, capped at `age_max`)
    pub age_per_day: f64,
    pub age_max: f64,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self {
            due_overdue_base: 12.0,
            due_overdue_per_day: 0.3,
            due_week_base: 6.0,
            due_week_per_day: 0.8,
            due_month_base: 3.0,
            due_month_per_day: 0.1,
            priority_low: 0.0,
            priority_medium: 1.0,
            priority_high: 2.0,
            project_low: -0.5,
            project_medium: 0.0,
            project_high: 0.5,
            age_per_day: 0.01,
            age_max: 2.0,
        }
    }
}

/// Application-wide configuration loaded from
/// `$XDG_CONFIG_HOME/task-manager/config.toml`.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub scoring: ScoringConfig,
    /// Number of days ahead shown by `next forecast` (default 90).
    pub forecast_horizon_days: u32,
    /// Default number of tasks shown by `next next` (default 10).
    pub next_count: usize,
}
