use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Result, bail};
use reqwest::blocking::{Client, ClientBuilder, Response};
use reqwest::header::HeaderMap;

pub static CLIENT: LazyLock<HttpClient> = LazyLock::new(HttpClient::new);

pub struct HttpClient {
    client: Client,
    retries: u32,
}

impl HttpClient {
    pub fn new() -> Self {
        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(30))
            // Random user agent
            .user_agent("curl/8.7.1")
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to construct HTTP Client");

        HttpClient { client, retries: 3 }
    }

    pub fn get(&self, url: &str, headers: Option<&HeaderMap>) -> Result<Response> {
        for _ in 0..self.retries {
            let mut request = self.client.get(url);
            if let Some(headers) = headers {
                request = request.headers(headers.clone());
            }

            let response = match request.send() {
                Ok(res) => res,
                Err(e) => {
                    log::warn!("Failed to send request. Retrying");
                    log::debug!("{}", e);
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            };

            match response.error_for_status() {
                Ok(res) => return Ok(res),
                Err(e) => {
                    log::warn!("Failed to send request. Retrying");
                    log::debug!("{}", e);
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            }
        }

        bail!("Failed to send request to {}", url)
    }
}
