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
    ) -> anyhow::Result<Option<SpotPriceResponse>> {
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
            Ok(day_ahead_prices) => Ok(Some(SpotPriceResponse {
                data: SpotPriceData {
                    market_prices_electricity: try_from_entsoe(day_ahead_prices)?,
                },
            })),

            Err(_e) => {
                let ack = serde_xml_rs::from_str::<EntsoeAcknowledgement>(&response_body)?;
                if ack.reason.code == "999" {
                    Ok(None)
                } else {
                    Err(anyhow::anyhow!("{:?}", ack.reason))
                }
            }
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct TimeInterval {
    #[serde(rename = "start", with = "rfc3339_without_seconds")]
    pub start: DateTime<Utc>,
    #[serde(rename = "end", with = "rfc3339_without_seconds")]
    pub end: DateTime<Utc>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct EntsoeDayAheadPrices {
    #[serde(rename = "TimeSeries", default)]
    pub time_series: Vec<DayAheadPricesTimeSeries>,
    #[expect(unused)]
    #[serde(rename = "type")]
    pub r#type: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct DayAheadPricesTimeSeries {
    #[serde(rename = "Period", default)]
    pub periods: Vec<DayAheadPricesPeriod>,
    #[allow(unused)]
    #[serde(rename = "currency_Unit.name")]
    pub currency_unit: String,
    #[allow(unused)]
    #[serde(rename = "price_Measure_Unit.name")]
    pub price_measure_unit: String,
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
    #[serde(rename = "position")]
    pub position: u64,
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

#[derive(Deserialize, Clone, Debug)]
pub struct EntsoeAcknowledgement {
    #[serde(rename = "Reason")]
    pub reason: EntsoeReason,
}

#[derive(Deserialize, Clone, Debug)]
pub struct EntsoeReason {
    #[serde(rename = "code")]
    pub code: String,
    #[serde(rename = "text")]
    #[allow(unused)]
    pub text: String,
}

fn get_datetime(
    start: DateTime<Utc>,
    position: u64,
    resolution: &str,
) -> anyhow::Result<DateTime<Utc>> {
    let position_i64: i64 = position
        .try_into()
        .unwrap_or_else(|_| panic!("Failed to  convert position {position} to i64"));

    let position_u32: u32 = position
        .try_into()
        .unwrap_or_else(|_| panic!("Failed to  convert position {position} to u32"));

    match resolution {
        "PT60M" => Ok(start + Duration::minutes(position_i64 * 60)),
        "PT15M" => Ok(start + Duration::minutes(position_i64 * 15)),
        "PT1M" => Ok(start + Duration::minutes(position_i64)),
        "P1D" => Ok(start + Duration::days(position_i64)),
        "P7D" => Ok(start + Duration::days(position_i64 * 7)),
        "P1M" => Ok(start
            .checked_add_months(Months::new(position_u32))
            .ok_or("Can't add 1 month")
            .map_err(|e| anyhow::anyhow!("{e:?}"))?),
        "P1Y" => Ok(start
            .checked_sub_months(Months::new(position_u32 * 12))
            .ok_or("Can't add 12 months")
            .map_err(|e| anyhow::anyhow!("{e:?}"))?),
        _ => Err(anyhow::anyhow!("Unknown resolution {resolution}")),
    }
}

fn get_start(
    start: DateTime<Utc>,
    position: u64,
    resolution: &str,
) -> anyhow::Result<DateTime<Utc>> {
    get_datetime(start, position - 1, resolution)
}

fn get_end(start: DateTime<Utc>, position: u64, resolution: &str) -> anyhow::Result<DateTime<Utc>> {
    get_datetime(start, position, resolution)
}

fn get_nr_positions(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    resolution: &str,
) -> anyhow::Result<u64> {
    let mut nr_positions = 0;

    while get_start(start, nr_positions + 1, resolution)? < end {
        nr_positions += 1;
    }

    Ok(nr_positions)
}

pub(crate) fn try_from_entsoe(source: EntsoeDayAheadPrices) -> anyhow::Result<Vec<SpotPrice>> {
    let mut prices: Vec<SpotPrice> = vec![];

    if source.time_series.is_empty() {
        return Ok(vec![]);
    }

    for time_serie in &source.time_series {
        for period in &time_serie.periods {
            let nr_of_expected_positions = get_nr_positions(
                period.time_interval.start,
                period.time_interval.end,
                &period.resolution,
            )
            .expect("Failed getting number of positions");

            let mut peekable_point_iter = period.points.iter().peekable();
            while let Some(point) = peekable_point_iter.next() {
                let mut current_position = point.position;
                // look ahead to check the next position to see if there are any gaps
                let next_position = if let Some(next_point) = peekable_point_iter.peek() {
                    next_point.position
                } else {
                    nr_of_expected_positions + 1
                };

                // ensure that at least one price is added or repeat the same price when there's a gap
                while current_position < next_position {
                    let start = get_start(
                        period.time_interval.start,
                        current_position,
                        &period.resolution,
                    )
                    .expect("Failed getting start time");

                    let end = get_end(
                        period.time_interval.start,
                        current_position,
                        &period.resolution,
                    )
                    .expect("Failed getting end time");

                    let market_price_per_kwh: f64 = point.price_amount.to_f64().unwrap() / 1_000.;

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

                    current_position += 1;
                }
            }
        }
    }

    Ok(prices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;
    use std::fs;

    #[test]
    fn deserialize_entsoe_day_ahead_prices() {
        let input: String = fs::read_to_string("./test-data/entsoe-day-ahead-prices.xml")
            .expect("Failed to read file");

        // act
        let response = serde_xml_rs::from_str::<EntsoeDayAheadPrices>(&input)
            .expect("Failed to deserialize xml response");

        assert_eq!(response.time_series[0].currency_unit, "EUR");
        assert_eq!(response.time_series[0].price_measure_unit, "MWH");
        assert_eq!(
            response.time_series[0].periods[0].time_interval.start,
            Utc.with_ymd_and_hms(2023, 2, 15, 23, 0, 0).unwrap()
        );
        assert_eq!(
            response.time_series[0].periods[0].time_interval.end,
            Utc.with_ymd_and_hms(2023, 2, 16, 23, 0, 0).unwrap()
        );
        assert_eq!(response.time_series[0].periods[0].resolution, "PT60M");
        assert_eq!(response.time_series[0].periods[0].points.len(), 24);
        assert_eq!(response.time_series[0].periods[0].points[0].position, 1);
        assert_eq!(
            response.time_series[0].periods[0].points[0].price_amount,
            dec!(91.02)
        );
    }

    #[test]
    fn deserialize_empty_entsoe_day_ahead_prices() {
        let input: String = fs::read_to_string("./test-data/entsoe-day-ahead-prices-empty.xml")
            .expect("Failed to read file");

        // act
        let response = serde_xml_rs::from_str::<EntsoeDayAheadPrices>(&input)
            .expect("Failed to deserialize xml response");

        assert_eq!(response.time_series[0].currency_unit, "EUR");
        assert_eq!(response.time_series[0].price_measure_unit, "MWH");
        assert_eq!(
            response.time_series[0].periods[0].time_interval.start,
            Utc.with_ymd_and_hms(2023, 2, 15, 23, 0, 0).unwrap()
        );
        assert_eq!(
            response.time_series[0].periods[0].time_interval.end,
            Utc.with_ymd_and_hms(2023, 2, 16, 23, 0, 0).unwrap()
        );
        assert_eq!(response.time_series[0].periods[0].resolution, "PT60M");
        assert_eq!(response.time_series[0].periods[0].points.len(), 0);
    }

    #[test]
    fn deserialize_entsoe_day_ahead_prices_with_multiple_timeseries() {
        let input: String =
            fs::read_to_string("./test-data/entsoe-day-ahead-prices-multiple-series.xml")
                .expect("Failed to read file");

        // act
        let response = serde_xml_rs::from_str::<EntsoeDayAheadPrices>(&input)
            .expect("Failed to deserialize xml response");

        assert_eq!(response.time_series.len(), 2);
    }

    #[test]
    fn day_ahead_to_spot_prices_entsoe_day_ahead_prices_with_multiple_timeseries() {
        let input: String =
            fs::read_to_string("./test-data/entsoe-day-ahead-prices-multiple-series.xml")
                .expect("Failed to read file");
        let response = serde_xml_rs::from_str::<EntsoeDayAheadPrices>(&input)
            .expect("Failed to deserialize xml response");

        // act
        let prices = try_from_entsoe(response).expect("Failed converting to internal type");

        assert_eq!(prices.len(), 47);
    }

    #[test]
    fn deserialize_entsoe_day_ahead_prices_with_multiple_periods() {
        let input: String =
            fs::read_to_string("./test-data/entsoe-day-ahead-prices-multiple-periods.xml")
                .expect("Failed to read file");

        // act
        let response = serde_xml_rs::from_str::<EntsoeDayAheadPrices>(&input)
            .expect("Failed to deserialize xml response");

        assert_eq!(response.time_series.len(), 1);
        assert_eq!(response.time_series[0].periods.len(), 2);
    }

    #[test]
    fn deserialize_entsoe_acknowledgement() {
        let input: String = fs::read_to_string("./test-data/entsoe-day-ahead-prices-no-data.xml")
            .expect("Failed to read file");

        // act
        let response = serde_xml_rs::from_str::<EntsoeAcknowledgement>(&input)
            .expect("Failed to deserialize xml response");

        assert_eq!(response.reason.code, "999");
        assert_eq!(
            response.reason.text,
            "No matching data found for Data item Day-ahead Prices [12.1.D] (10YNL----------L, 10YNL----------L) and interval 2023-02-19T23:00:00.000Z/2023-02-20T23:00:00.000Z."
        );
    }

    #[test]
    fn do_not_deserialize_entsoe_acknowledgement_as_day_ahead_prices() {
        let input: String = fs::read_to_string("./test-data/entsoe-day-ahead-prices-no-data.xml")
            .expect("Failed to read file");

        // act
        let is_err = serde_xml_rs::from_str::<EntsoeDayAheadPrices>(&input).is_err();

        assert!(is_err);
    }

    #[test]
    fn deserialize_response_with_gap_in_middle_should_repeat_the_price_before_the_gap() {
        let input: String = fs::read_to_string("./test-data/entsoe-day-ahead-prices-gap.xml")
            .expect("Failed to read file");

        // act
        let response = serde_xml_rs::from_str::<EntsoeDayAheadPrices>(&input)
            .expect("Failed to deserialize xml response");

        let prices = try_from_entsoe(response).expect("Failed converting to internal type");

        assert_eq!(prices.len(), 24);
        // first price
        assert_eq!(
            prices[0].from,
            Utc.with_ymd_and_hms(2025, 4, 1, 22, 0, 0).unwrap()
        );
        assert_eq!(
            prices[0].till,
            Utc.with_ymd_and_hms(2025, 4, 1, 23, 0, 0).unwrap()
        );
        assert_eq!(prices[0].market_price, 0.06563);

        // before gap
        assert_eq!(
            prices[19].from,
            Utc.with_ymd_and_hms(2025, 4, 2, 17, 0, 0).unwrap()
        );
        assert_eq!(
            prices[19].till,
            Utc.with_ymd_and_hms(2025, 4, 2, 18, 0, 0).unwrap()
        );
        assert_eq!(prices[19].market_price, 0.114);

        // filled gap by repeating last price
        assert_eq!(
            prices[20].from,
            Utc.with_ymd_and_hms(2025, 4, 2, 18, 0, 0).unwrap()
        );
        assert_eq!(
            prices[20].till,
            Utc.with_ymd_and_hms(2025, 4, 2, 19, 0, 0).unwrap()
        );
        assert_eq!(prices[20].market_price, 0.114);

        // after gap
        assert_eq!(
            prices[21].from,
            Utc.with_ymd_and_hms(2025, 4, 2, 19, 0, 0).unwrap()
        );
        assert_eq!(
            prices[21].till,
            Utc.with_ymd_and_hms(2025, 4, 2, 20, 0, 0).unwrap()
        );
        assert_eq!(prices[21].market_price, 0.09606999999999999);

        // last price
        assert_eq!(
            prices[23].from,
            Utc.with_ymd_and_hms(2025, 4, 2, 21, 0, 0).unwrap()
        );
        assert_eq!(
            prices[23].till,
            Utc.with_ymd_and_hms(2025, 4, 2, 22, 0, 0).unwrap()
        );
        assert_eq!(prices[23].market_price, 0.07143000000000001);
    }

    #[test]
    fn deserialize_response_with_gap_at_end_should_repeat_the_price_before_the_gap() {
        let input: String =
            fs::read_to_string("./test-data/entsoe-day-ahead-prices-gap-at-end.xml")
                .expect("Failed to read file");

        // act
        let response = serde_xml_rs::from_str::<EntsoeDayAheadPrices>(&input)
            .expect("Failed to deserialize xml response");

        let prices = try_from_entsoe(response).expect("Failed converting to internal type");

        assert_eq!(prices.len(), 24);

        // last price in response
        assert_eq!(
            prices[22].from,
            Utc.with_ymd_and_hms(2025, 4, 2, 20, 0, 0).unwrap()
        );
        assert_eq!(
            prices[22].till,
            Utc.with_ymd_and_hms(2025, 4, 2, 21, 0, 0).unwrap()
        );
        assert_eq!(prices[22].market_price, 0.07143000000000001);

        // filled gap at end by repeating last price
        assert_eq!(
            prices[23].from,
            Utc.with_ymd_and_hms(2025, 4, 2, 21, 0, 0).unwrap()
        );
        assert_eq!(
            prices[23].till,
            Utc.with_ymd_and_hms(2025, 4, 2, 22, 0, 0).unwrap()
        );
        assert_eq!(prices[23].market_price, 0.07143000000000001);
    }

    #[test]
    fn get_nr_positions_returns_zero_if_start_and_end_are_equal() {
        // act
        let nr_positions = get_nr_positions(
            Utc.with_ymd_and_hms(2025, 4, 1, 22, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2025, 4, 1, 22, 0, 0).unwrap(),
            "PT60M",
        )
        .expect("Failed getting number of positions");

        assert_eq!(nr_positions, 0);
    }

    #[test]
    fn get_nr_positions_returns_24_for_a_full_day() {
        // act
        let nr_positions = get_nr_positions(
            Utc.with_ymd_and_hms(2025, 4, 1, 22, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2025, 4, 2, 22, 0, 0).unwrap(),
            "PT60M",
        )
        .expect("Failed getting number of positions");

        assert_eq!(nr_positions, 24);
    }

    #[test]
    fn get_nr_positions_returns_23_for_winter_to_summer_time() {
        // act
        let nr_positions = get_nr_positions(
            Utc.with_ymd_and_hms(2016, 3, 26, 23, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2016, 3, 27, 22, 0, 0).unwrap(),
            "PT60M",
        )
        .expect("Failed getting number of positions");

        assert_eq!(nr_positions, 23);
    }
}
