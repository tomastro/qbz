//! Reproduce EXACTLY what cdda_qt does, and say what comes back.
#[tokio::main]
async fn main() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let id = std::env::args().nth(1)
        .unwrap_or_else(|| "BeNBMsD8Du5NO2W61Yk.B2jwwIs-".to_string());
    let url = qbz_disc::discid::lookup_url(&id);
    println!("URL: {url}");

    let client = reqwest::Client::builder()
        .user_agent(concat!("QBZ/", env!("CARGO_PKG_VERSION"), " (https://github.com/vicrodh/qbz)"))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("client");

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => { println!("SEND FAILED: {e}"); return; }
    };
    let status = resp.status();
    println!("HTTP {status}");
    let text = resp.text().await.unwrap_or_default();
    println!("body: {} bytes", text.len());
    println!("first 300: {}", &text[..text.len().min(300)]);

    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => { println!("NOT JSON: {e}"); return; }
    };
    println!("top-level keys: {:?}", v.as_object().map(|o| o.keys().collect::<Vec<_>>()));
    let releases = v.get("releases").and_then(|r| r.as_array());
    println!("releases: {:?}", releases.map(|r| r.len()));
    if let Some(r0) = releases.and_then(|r| r.first()) {
        println!("  release keys: {:?}", r0.as_object().map(|o| o.keys().collect::<Vec<_>>()));
        println!("  title: {:?}", r0.get("title"));
        let media = r0.get("media").and_then(|m| m.as_array());
        println!("  media: {:?}", media.map(|m| m.len()));
        if let Some(m0) = media.and_then(|m| m.iter().find(|m| m.get("tracks").is_some())) {
            let t = m0.get("tracks").and_then(|t| t.as_array());
            println!("  tracks: {:?}", t.map(|t| t.len()));
        } else {
            println!("  NO media entry has `tracks`  <-- aqui muere el parseo");
        }
    }
}
