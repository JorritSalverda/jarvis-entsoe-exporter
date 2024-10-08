mod bigquery_client;
mod entsoe_client;
mod exporter_service;
mod state_client;
mod types;

use bigquery_client::BigqueryClient;
use chrono::Utc;
use entsoe_client::EntsoeClient;
use exporter_service::ExporterService;
use state_client::StateClient;
use std::error::Error;

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn Error>> {
    json_env_logger::init();

    let bigquery_client = BigqueryClient::from_env().await?;
    let spot_price_client = EntsoeClient::from_env()?;
    let state_client = StateClient::from_env().await?;

    let exporter_service =
        ExporterService::from_env(bigquery_client, spot_price_client, state_client)?;

    exporter_service.run(Utc::now()).await
}

#[cfg(test)]
#[ctor::ctor]
fn init() {
    json_env_logger::init();
}
