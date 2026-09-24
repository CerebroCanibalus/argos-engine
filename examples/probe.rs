//! One-off probe: can plain reqwest (rustls) with a browser UA reach
//! html.duckduckgo.com, or does the endpoint require TLS impersonation?
//!
//! Run with: cargo run --example probe

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

#[tokio::main]
async fn main() {
    let client = reqwest::Client::builder()
        .user_agent(UA)
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("client");

    // DDG html endpoint (POST, same payload ddgs uses)
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
                "DDG status={status} len={} result__a_hits={hits}",
                body.len()
            );
            if hits == 0 {
                let snippet: String = body.chars().take(300).collect();
                println!("DDG body head: {snippet}");
            }
        }
        Err(ex) => println!("DDG transport error: {ex}"),
    }

    // Mojeek sanity (already verified via PowerShell, re-verified through reqwest)
    match client
        .get("https://www.mojeek.com/search?q=rust+programming+language")
        .send()
        .await
    {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            let hits = body.matches("class=\"s\"").count();
            println!("MOJEEK status={status} len={} p.s_hits={hits}", body.len());
        }
        Err(ex) => println!("MOJEEK transport error: {ex}"),
    }

    // Bing: exact provider request - does rustls get the real results page?
    match client
        .get("https://www.bing.com/search")
        .header(
            "Cookie",
            "_EDGE_CD=m=en-us&u=en-us; _EDGE_S=mkt=en-us&ui=en-us",
        )
        .form(&[
            ("q", "rust programming language"),
            ("pq", "rust programming language"),
            ("cc", "en"),
        ])
        .send()
        .await
    {
        Ok(r) => {
            let status = r.status();
            let final_url = r.url().to_string();
            let body = r.text().await.unwrap_or_default();
            let hits = body.matches("b_algo").count();
            let challenge = body.contains("challenge") || body.contains("CAPTCHA");
            let len = body.len();
            println!(
                "BING status={status} len={len} b_algo={hits} challenge={challenge} url={final_url}"
            );
            let head: String = body.chars().take(300).collect();
            println!("BING head: {head}");
        }
        Err(ex) => println!("BING transport error: {ex}"),
    }

    // Bing with redirects DISABLED: is /search itself a302 for us?
    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client");
    match no_redirect
        .get("https://www.bing.com/search")
        .header(
            "Cookie",
            "_EDGE_CD=m=en-us&u=en-us; _EDGE_S=mkt=en-us&ui=en-us",
        )
        .form(&[
            ("q", "rust programming language"),
            ("pq", "rust programming language"),
            ("cc", "en"),
        ])
        .send()
        .await
    {
        Ok(r) => {
            let status = r.status();
            let version = format!("{:?}", r.version());
            let location = r
                .headers()
                .get("location")
                .map(|v| v.to_str().unwrap_or("?").to_string())
                .unwrap_or_default();
            println!("BING no-redirect status={status} version={version} location={location}");
        }
        Err(ex) => println!("BING no-redirect transport error: {ex}"),
    }
}
