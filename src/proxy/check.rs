//! The proxy's Test: whether it answers, and what address the outside world sees through it.
//! That's asked of an IP-echo service (ipify, which answers with the caller's address and
//! nothing else), itself an outside request, made only when an admin presses Test. Nothing
//! else goes with it: no setting, nothing about the household.

use std::time::Duration;

use super::*;

/// Where the Test asks which address it comes from.
pub const ECHO: &str = "https://api.ipify.org";

/// Tests `proxy`: Ok with what to tell the admin when the echo service answered through it,
/// Err with why not. Whatever is said names the proxy by its address, never its login.
pub async fn test(proxy: &Proxy, echo: &str) -> Result<String, String> {
    let echo_host = reqwest::Url::parse(echo)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| echo.to_owned());
    let http = via(reqwest::Client::builder(), proxy, None)
        .and_then(|b| {
            Ok(b.timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("melete/", env!("CARGO_PKG_VERSION")))
                .build()?)
        })
        .map_err(|_| format!("{} can't be used as a proxy.", proxy.address()))?;
    let answer = match http.get(echo).send().await {
        Ok(answer) => answer,
        Err(err) => {
            let why = std::iter::successors(Some(&err as &dyn std::error::Error), |e| e.source())
                .find_map(|e| e.downcast_ref::<Unreachable>());
            return Err(match why {
                Some(Unreachable::Down(at)) => format!(
                    "The proxy at {at} doesn't answer. Check its host, port and type, and that it's running."
                ),
                Some(Unreachable::Through(at)) => format!(
                    "The proxy at {at} answered, but {echo_host} couldn't be reached through it. Check its type, and its username and password if it needs them."
                ),
                _ => format!("{echo_host} didn't answer through the proxy in time."),
            });
        }
    };
    if !answer.status().is_success() {
        return Err(format!(
            "Through the proxy, {echo_host} answered {}. An HTTP proxy that wants a login answers 407.",
            answer.status()
        ));
    }
    let text = answer.text().await.unwrap_or_default();
    match text.trim().parse::<std::net::IpAddr>() {
        Ok(seen) => Ok(format!(
            "The proxy answered. The outside world sees {seen}, as {echo_host} saw it."
        )),
        Err(_) => Err(format!(
            "Through the proxy, {echo_host} answered with something that isn't an address."
        )),
    }
}
