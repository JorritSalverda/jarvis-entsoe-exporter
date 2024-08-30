use crate::types::{SpotPrice, SpotPriceData, SpotPriceResponse};
use chrono::{DateTime, Duration, Months, Utc};
use rust_decimal::{prelude::ToPrimitive, Decimal};
use serde::Deserialize;
use std::env;
use uuid::Uuid;

const EIC_CODE: &str = "10YNL----------L";

pub struct EntsoeClient {
    api_token: String,
    client: reqwest::Client,
}

impl EntsoeClient {
    pub fn new(api_token: String, client: reqwest::Client) -> Self {
        Self { api_token, client }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self::new(
            env::var("ENTSOE_API_TOKEN")?,
            reqwest::Client::new(),
        ))
    }

    pub async fn get_spot_prices(
        &self,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
    ) -> anyhow::Result<SpotPriceResponse> {
        let document_type = "A44".to_string();
        let period_start = period_start.format("%Y%m%d%H%M").to_string();
        let period_end = period_end.format("%Y%m%d%H%M").to_string();

        let url = format!("https://web-api.tp.entsoe.eu/api?documentType={document_type}&in_Domain={EIC_CODE}&out_Domain={EIC_CODE}&periodStart={period_start}&periodEnd={period_end}");

        log::info!("Fetching day ahead prices from {}...", url);

        let response = self
            .client
            .get(format!("{}&securityToken={}", url, self.api_token))
            .send()
            .await?;

        let status_code = response.status();
        let response_body = response.text().await?;

        if !status_code.is_success() {
            log::warn!(
                "Status code {status_code} indicates failure: {}",
                response_body
            );
            return Err(anyhow::anyhow!(
                "Status code {status_code} indicates failure"
            ));
        }

        match serde_xml_rs::from_str::<EntsoeDayAheadPrices>(&response_body) {
            Ok(day_ahead_prices) => Ok(SpotPriceResponse {
                data: SpotPriceData {
                    market_prices_electricity: day_ahead_to_spot_prices(
                        &day_ahead_prices.time_series,
                    )?,
                },
            }),
            Err(e) => Err(anyhow::anyhow!("{e:?}")),
        }
    }
}

fn day_ahead_to_spot_prices(
    day_ahead_prices: &[DayAheadPricesTimeSeries],
) -> anyhow::Result<Vec<SpotPrice>> {
    if day_ahead_prices.is_empty() {
        return Ok(vec![]);
    };

    let mut prices: Vec<SpotPrice> = vec![];

    for day_ahead_price in day_ahead_prices {
        let mut start = day_ahead_price.period.time_interval.start;
        for point in &day_ahead_price.period.points {
            let end = get_end(start, &day_ahead_price.period.resolution)
                .expect("Failed getting end time");

            let market_price_per_kwh: f64 = point.price_amount.to_f64().unwrap() / 1000.;

            prices.push(SpotPrice {
                id: Some(Uuid::new_v4().to_string()),
                source: Some("entso-e".into()),
                from: start,
                till: end,
                market_price: market_price_per_kwh,
                market_price_tax: market_price_per_kwh * 0.21,
                sourcing_markup_price: 0.0182, // frank energie
                energy_tax_price: 0.1316,      // 2024
            });

            start = end;
        }
    }

    Ok(prices)
}

fn get_end(start: DateTime<Utc>, resolution: &str) -> anyhow::Result<DateTime<Utc>> {
    match resolution {
        "PT60M" => Ok(start + Duration::minutes(60)),
        "PT15M" => Ok(start + Duration::minutes(15)),
        "PT1M" => Ok(start + Duration::minutes(1)),
        "P1D" => Ok(start + Duration::days(1)),
        "P7D" => Ok(start + Duration::days(7)),
        "P1M" => Ok(start
            .checked_add_months(Months::new(1))
            .ok_or("Can't add 1 month")
            .map_err(|e| anyhow::anyhow!("{e:?}"))?),
        "P1Y" => Ok(start
            .checked_sub_months(Months::new(12))
            .ok_or("Can't add 12 months")
            .map_err(|e| anyhow::anyhow!("{e:?}"))?),
        _ => Err(anyhow::anyhow!("Unknown resolution {resolution}")),
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct TimeInterval {
    #[serde(rename = "start", with = "rfc3339_without_seconds")]
    pub start: DateTime<Utc>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct EntsoeDayAheadPrices {
    #[serde(rename = "TimeSeries", default)]
    pub time_series: Vec<DayAheadPricesTimeSeries>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct DayAheadPricesTimeSeries {
    #[serde(rename = "Period")]
    pub period: DayAheadPricesPeriod,
}

#[derive(Deserialize, Clone, Debug)]
pub struct DayAheadPricesPeriod {
    #[serde(rename = "timeInterval")]
    pub time_interval: TimeInterval,
    #[serde(rename = "resolution")]
    pub resolution: String,
    #[serde(rename = "Point", default)]
    pub points: Vec<DayAheadPricesPoint>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct DayAheadPricesPoint {
    #[serde(rename = "price.amount", with = "rust_decimal::serde::str")]
    pub price_amount: Decimal,
}

mod rfc3339_without_seconds {
    use chrono::{DateTime, NaiveDateTime, Utc};
    use serde::{self, Deserialize, Deserializer};

    const FORMAT: &str = "%Y-%m-%dT%H:%MZ";

    pub fn deserialize<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;

        Ok(NaiveDateTime::parse_from_str(&s, FORMAT)
            .map_err(serde::de::Error::custom)?
            .and_utc())
    }
}
