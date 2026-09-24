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
}
