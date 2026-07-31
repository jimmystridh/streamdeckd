//! Real-time departures from Västtrafik's journey-planner API.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use streamdeck_core::config::VasttrafikConfig;
use streamdeck_core::integrations::departures::{self, DepartureBoard, StopDepartures};
use tokio::sync::Mutex;

use super::http::{HttpClient, HttpError};
use super::timeouts;

const TOKEN_URL: &str = "https://www.vasttrafik.se/api/token/external/new";
const API_BASE: &str = "https://ext-api.vasttrafik.se/pr/v4-int";

#[derive(Clone)]
pub struct Client {
    http: HttpClient,
    token: Arc<Mutex<Option<CachedToken>>>,
}

#[derive(Clone)]
struct CachedToken {
    value: String,
    valid_until: Instant,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenResponse {
    token: String,
    expires_in: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum VasttrafikError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error(transparent)]
    Parse(#[from] streamdeck_core::integrations::ParseError),
    #[error("Västtrafik returned an invalid access token")]
    InvalidToken,
}

impl Client {
    pub fn new(http: HttpClient) -> Self {
        Self {
            http,
            token: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn fetch(
        &self,
        config: &VasttrafikConfig,
        now: DateTime<Utc>,
    ) -> Result<DepartureBoard, VasttrafikError> {
        let mut stops = Vec::with_capacity(config.stops.len());
        for stop in &config.stops {
            let mut departures = self.fetch_stop(&stop.label, &stop.gid, now).await?;
            departures.retain_route(&stop.line, &stop.direction);
            stops.push(departures);
        }
        Ok(DepartureBoard { stops })
    }

    async fn fetch_stop(
        &self,
        label: &str,
        gid: &str,
        now: DateTime<Utc>,
    ) -> Result<StopDepartures, VasttrafikError> {
        let token = self.access_token().await?;
        match self.fetch_stop_with_token(label, gid, now, &token).await {
            Err(VasttrafikError::Http(HttpError::Unauthorized { .. })) => {
                *self.token.lock().await = None;
                let token = self.access_token().await?;
                self.fetch_stop_with_token(label, gid, now, &token).await
            }
            result => result,
        }
    }

    async fn fetch_stop_with_token(
        &self,
        label: &str,
        gid: &str,
        now: DateTime<Utc>,
        token: &str,
    ) -> Result<StopDepartures, VasttrafikError> {
        let url = format!("{API_BASE}/stop-areas/{gid}/departures?limit=40&includeOccupancy=false");
        let authorization = format!("Bearer {token}");
        let response = self
            .http
            .get(
                &url,
                &[
                    ("Accept", "application/json"),
                    ("Authorization", &authorization),
                ],
                timeouts::VASTTRAFIK,
            )
            .await?;
        Ok(departures::parse(label, gid, &response.body, now)?)
    }

    async fn access_token(&self) -> Result<String, VasttrafikError> {
        let mut cached = self.token.lock().await;
        if let Some(token) = cached.as_ref() {
            if token.valid_until > Instant::now() + Duration::from_secs(60) {
                return Ok(token.value.clone());
            }
        }

        let response = self
            .http
            .get(
                TOKEN_URL,
                &[("Accept", "application/json")],
                timeouts::VASTTRAFIK,
            )
            .await?;
        let token: TokenResponse = serde_json::from_str(&response.body).map_err(|source| {
            streamdeck_core::integrations::ParseError::Json {
                integration: "vasttrafik-token",
                source,
            }
        })?;
        if token.token.is_empty() || token.expires_in <= 60 {
            return Err(VasttrafikError::InvalidToken);
        }
        let value = token.token;
        *cached = Some(CachedToken {
            value: value.clone(),
            valid_until: Instant::now() + Duration::from_secs(token.expires_in),
        });
        Ok(value)
    }
}
