use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SpotPriceRequest {
    pub query: String,
    pub variables: SpotPriceRequestVariables,
    pub operation_name: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SpotPriceRequestVariables {
    pub start_date: String,
    pub end_date: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SpotPriceResponse {
    pub data: SpotPriceData,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SpotPriceData {
    pub market_prices_electricity: Vec<SpotPrice>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SpotPrice {
    pub id: Option<String>,
    pub source: Option<String>,
    pub from: DateTime<Utc>,
    pub till: DateTime<Utc>,
    pub market_price: f64,
    pub market_price_tax: f64,
    pub sourcing_markup_price: f64,
    pub energy_tax_price: f64,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub future_spot_prices: Vec<SpotPrice>,
    pub last_from: DateTime<Utc>,
}
