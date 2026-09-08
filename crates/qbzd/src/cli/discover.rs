// crates/qbzd/src/cli/discover.rs — `qbzd discover [SECTION] [--genre IDS]
// [--tag TAG] [--release-type RT] [--type FT] [--limit N] [--ids] [--json]`.
// Exposes the Discover rails so a client can replicate the Discover page. The
// human/--ids views walk the payload's items arrays generically; --json is the
// exact contract.
use crate::cli::browse::{collect_ids, render};
use crate::cli::client::ApiClient;
use crate::paths::ProfileRoots;

#[allow(clippy::too_many_arguments)]
pub async fn discover(
    host: Option<String>,
    section: Option<String>,
    genre: Option<String>,
    tag: Option<String>,
    release_type: Option<String>,
    kind: Option<String>,
    limit: u32,
    ids: bool,
    json: bool,
    roots: &ProfileRoots,
) -> i32 {
    let sec = section.unwrap_or_else(|| "index".to_string());
    let mut path = format!("/api/discover?section={}&limit={}", urlencoding::encode(&sec), limit);
    if let Some(g) = genre.filter(|s| !s.is_empty()) {
        path.push_str(&format!("&genre={}", urlencoding::encode(&g)));
    }
    if let Some(t) = tag.filter(|s| !s.is_empty()) {
        path.push_str(&format!("&tag={}", urlencoding::encode(&t)));
    }
    if let Some(r) = release_type.filter(|s| !s.is_empty()) {
        path.push_str(&format!("&release_type={}", urlencoding::encode(&r)));
    }
    if let Some(k) = kind.filter(|s| !s.is_empty()) {
        path.push_str(&format!("&type={}", urlencoding::encode(&k)));
    }

    let client = ApiClient::new(host, roots);
    match client.get(&path).await {
        Ok(v) => {
            if json {
                println!("{}", serde_json::to_string(&v).unwrap_or_default());
            } else if ids {
                for id in collect_ids(&v) {
                    println!("{id}");
                }
            } else {
                print!("{}", render(&v));
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            e.exit_code()
        }
    }
}
