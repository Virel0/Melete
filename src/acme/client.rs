//! instant-acme's requests, sent with an HTTP client from the proxy module like every other
//! outside service's: direct, with a timeout, no redirects followed, and Melete's own
//! User-Agent in place of the library's.

use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::BodyExt;
use instant_acme::{BodyWrapper, BytesResponse, Error, HttpClient};

/// The longest answer taken from the CA: a certificate chain is a few kilobytes.
const MOST: usize = 1 << 20;

/// An HTTP client for the CA.
pub struct Client(crate::proxy::Http);

impl Client {
    pub fn boxed() -> anyhow::Result<Box<dyn HttpClient>> {
        let http = crate::proxy::Http::through(false, |b| {
            b.timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("melete/", env!("CARGO_PKG_VERSION")))
        })?;
        Ok(Box::new(Client(http)))
    }
}

fn other(why: String) -> Error {
    Error::Other(why.into())
}

impl HttpClient for Client {
    fn request(
        &self,
        request: http::Request<BodyWrapper<Bytes>>,
    ) -> Pin<Box<dyn Future<Output = Result<BytesResponse, Error>> + Send>> {
        let http = self.0.clone();
        Box::pin(async move {
            let (asked, body) = request.into_parts();
            let body = match body.collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(never) => match never {},
            };
            let mut sending = http.request(asked.method, asked.uri.to_string());
            for (name, value) in &asked.headers {
                if name != http::header::USER_AGENT {
                    sending = sending.header(name, value);
                }
            }
            let answer = sending.body(body).send().await.map_err(|err| {
                other(format!("the CA didn't answer: {}", crate::proxy::said(err)))
            })?;
            let mut made = http::Response::builder().status(answer.status());
            for (name, value) in answer.headers() {
                made = made.header(name, value);
            }
            let (parts, ()) = made.body(())?.into_parts();
            let mut body = Vec::new();
            let mut answer = answer;
            while let Some(chunk) = answer
                .chunk()
                .await
                .map_err(|err| other(format!("the CA's answer broke off: {err}")))?
            {
                body.extend_from_slice(&chunk);
                if body.len() > MOST {
                    return Err(other("the CA's answer is far too long".to_owned()));
                }
            }
            Ok(BytesResponse {
                parts,
                body: Box::new(Bytes::from(body)),
            })
        })
    }
}
