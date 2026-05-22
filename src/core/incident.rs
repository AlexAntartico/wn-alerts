use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incident {
    pub provider: String,
    pub id: String,
    pub title: String,
    pub description: String,
    pub link: String,
    pub occurred_at: String,
}
