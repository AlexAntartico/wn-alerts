use async_trait::async_trait;

use crate::core::incident::Incident;
use crate::error::AppError;

#[async_trait]
pub trait StatusProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn check(&self, client: &reqwest::Client) -> Result<Vec<Incident>, AppError>;
}
