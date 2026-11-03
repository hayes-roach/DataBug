use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanNode {
    pub label: String,
    pub detail: String,
    pub total_cost: Option<f64>,
    pub startup_cost: Option<f64>,
    pub rows: Option<f64>,
    pub actual_ms: Option<f64>,
    pub children: Vec<PlanNode>,
}

impl PlanNode {
    pub fn max_cost(&self) -> f64 {
        let own = self.total_cost.or(self.actual_ms).unwrap_or(0.0);
        self.children.iter().map(|c| c.max_cost()).fold(own, f64::max)
    }
}

/// A query plan: a parsed tree when the engine's format is understood, plus
/// the original text in the database's native format (JSON, XML or text).
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub root: Option<PlanNode>,
    pub raw: String,
    pub format: &'static str,
}
