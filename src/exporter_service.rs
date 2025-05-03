use crate::bigquery_client::BigqueryClient;
use crate::entsoe_client::EntsoeClient;
use crate::state_client::StateClient;
use crate::types::*;
use chrono::{DateTime, Duration, DurationRound, TimeDelta, Utc};
use log::info;
use std::error::Error;
use tokio_retry::strategy::{jitter, ExponentialBackoff};
use tokio_retry::Retry;

pub struct ExporterServiceConfig {
    bigquery_client: BigqueryClient,
    spot_price_client: EntsoeClient,
    state_client: StateClient,
}

impl ExporterServiceConfig {
    pub fn new(
        bigquery_client: BigqueryClient,
        spot_price_client: EntsoeClient,
        state_client: StateClient,
    ) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            bigquery_client,
            spot_price_client,
            state_client,
        })
    }

    pub fn from_env(
        bigquery_client: BigqueryClient,
        spot_price_client: EntsoeClient,
        state_client: StateClient,
    ) -> Result<Self, Box<dyn Error>> {
        Self::new(bigquery_client, spot_price_client, state_client)
    }
}

pub struct ExporterService {
    config: ExporterServiceConfig,
}

impl ExporterService {
    pub fn new(config: ExporterServiceConfig) -> Self {
        Self { config }
    }

    pub fn from_env(
        bigquery_client: BigqueryClient,
        spot_price_client: EntsoeClient,
        state_client: StateClient,
    ) -> Result<Self, Box<dyn Error>> {
        Ok(Self::new(ExporterServiceConfig::from_env(
            bigquery_client,
            spot_price_client,
            state_client,
        )?))
    }

    pub async fn run(&self) -> Result<(), Box<dyn Error>> {
        let now: DateTime<Utc> = Utc::now();

        info!("Initalizing BigQuery table...");
        self.config.bigquery_client.init_table().await?;

        info!("Reading previous state...");
        let state = self.config.state_client.read_state()?;

        let add_time_delta_fn = |input: DateTime<Utc>, delta: TimeDelta| -> DateTime<Utc> {
            (input
                .with_timezone(&chrono_tz::Tz::Europe__Amsterdam)
                .duration_trunc(Duration::days(1))
                .unwrap()
                + delta)
                .with_timezone(&Utc)
        };

        let start_date: DateTime<Utc> = if let Some(state) = &state {
            add_time_delta_fn(state.last_from, Duration::days(0))
        } else {
            add_time_delta_fn(Utc::now(), Duration::days(0))
        };

        let mut period_start: DateTime<Utc> = start_date;
        let mut future_spot_prices: Vec<SpotPrice> = vec![];
        let mut last_till: Option<DateTime<Utc>> = None;

        let end_of_tomorrow = add_time_delta_fn(Utc::now(), Duration::days(2));

        loop {
            if Utc::now() - now > Duration::seconds(200) {
                info!("Job ran for more than 200 seconds, finished fetching data");
                break;
            }

            if period_start >= end_of_tomorrow {
                info!("Next start {period_start} >= {end_of_tomorrow}, finished fetching data");
                break;
            }

            let period_end: DateTime<Utc> = add_time_delta_fn(period_start, Duration::days(1));

            info!(
                "Retrieving day-ahead prices between {} and {}...",
                period_start, period_end
            );

            let Some(spot_price_response) = Retry::spawn(
                ExponentialBackoff::from_millis(100).map(jitter).take(3),
                || {
                    self.config
                        .spot_price_client
                        .get_spot_prices(period_start, period_end)
                },
            )
            .await?
            else {
                info!("Did not receive spot prices, trying again later");
                break;
            };

            let retrieved_spot_prices = spot_price_response.data.market_prices_electricity;
            info!("Retrieved {} day-ahead prices", retrieved_spot_prices.len());

            info!(
                "Storing retrieved day-ahead prices between {} and {}...",
                period_start, period_end
            );

            // reset spot prices to store in state
            if !retrieved_spot_prices.is_empty() {
                future_spot_prices = vec![];
            }

            for spot_price in &retrieved_spot_prices {
                info!("{:?}", spot_price);
                if spot_price.till > now {
                    future_spot_prices.push(spot_price.clone());
                }

                let write_spot_price = if let Some(st) = &state {
                    spot_price.from > st.last_from
                } else {
                    true
                };

                if write_spot_price {
                    Retry::spawn(
                        ExponentialBackoff::from_millis(100).map(jitter).take(3),
                        || self.config.bigquery_client.insert_spot_price(spot_price),
                    )
                    .await?;
                    last_till = Some(spot_price.till);
                } else {
                    info!("Skipping writing to BigQuery, already present");
                }
            }

            period_start = period_end;
        }

        if last_till.is_some() {
            info!("Writing new state...");
            let new_state = State {
                future_spot_prices,
                last_from: last_till.unwrap(),
            };

            self.config.state_client.store_state(&new_state).await?;
        }

        Ok(())
    }
}
