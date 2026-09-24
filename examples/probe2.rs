//! Probe primp (browser impersonation) against the keyless endpoints.
//!
//! Run with: cargo run --example probe2
//! The compiler guides the exact builder API on the first run.

#[tokio::main]
async fn main() {
    let client = primp::Client::builder()
        .impersonate(primp::Impersonate::ChromeV153)
        .impersonate_os(primp::ImpersonateOS::Windows)
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("primp client");

    // Bing: same request the provider makes.
    let resp = client
        .get("https://www.bing.com/search")
        .query(&[
            ("q", "rust programming language"),
            ("pq", "rust programming language"),
            ("cc", "en"),
        ])
        .header(
            "Cookie",
            "_EDGE_CD=m=en-us&u=en-us; _EDGE_S=mkt=en-us&ui=en-us",
        )
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let url = r.url().to_string();
            let body = r.text().await.unwrap_or_default();
            let hits = body.matches("b_algo").count();
            println!(
                "BING-PRIMP status={status} url={url} len={} b_algo={hits}",
                body.len()
            );
        }
        Err(ex) => println!("BING-PRIMP error: {ex}"),
    }

    // DuckDuckGo: POST form like the provider.
    let resp = client
        .post("https://html.duckduckgo.com/html/")
        .form(&[
            ("q", "rust programming language"),
            ("b", ""),
            ("l", "us-en"),
        ])
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            let hits = body.matches("result__a").count();
            println!(
                "DDG-PRIMP status={status} len={} result__a={hits}",
                body.len()
            );
        }
        Err(ex) => println!("DDG-PRIMP error: {ex}"),
    }

    // Brave: was429 with plain clients - does impersonation pass?
    let resp = client
        .get("https://search.brave.com/search")
        .query(&[("q", "rust programming language"), ("source", "web")])
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            let hits = body.matches("data-type=\"web\"").count();
            println!(
                "BRAVE-PRIMP status={status} len={} data-web={hits}",
                body.len()
            );
            if hits > 0 {
                std::fs::write("tests/fixtures/brave.html", &body).expect("save brave fixture");
                println!("BRAVE fixture saved to tests/fixtures/brave.html");
            }
        }
        Err(ex) => println!("BRAVE-PRIMP error: {ex}"),
    }

    // Mojeek: was CAPTCHA with plain clients - does impersonation pass?
    let resp = client
        .get("https://www.mojeek.com/search")
        .query(&[("q", "rust programming language")])
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            let hits = body.matches("class=\"s\"").count();
            let captcha = body.to_lowercase().contains("captcha");
            let len = body.len();
            println!("MOJEEK-PRIMP status={status} len={len} p.s={hits} captcha={captcha}");
        }
        Err(ex) => println!("MOJEEK-PRIMP error: {ex}"),
    }
}
