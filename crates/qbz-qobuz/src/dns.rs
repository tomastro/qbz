use reqwest::dns::{Name, Resolve, Resolving};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::time::Duration;

/// Tier 1: C getaddrinfo forcing AF_INET (IPv4)
fn getaddrinfo_ipv4(host: &str) -> Vec<SocketAddr> {
    use std::ffi::CString;

    let c_host = match CString::new(host) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = libc::AF_INET;
    hints.ai_socktype = libc::SOCK_STREAM;

    let mut res: *mut libc::addrinfo = std::ptr::null_mut();
    let ret = unsafe { libc::getaddrinfo(c_host.as_ptr(), std::ptr::null(), &hints, &mut res) };
    if ret != 0 || res.is_null() {
        return Vec::new();
    }

    let mut addrs = Vec::new();
    let mut curr = res;
    while !curr.is_null() {
        unsafe {
            let ai = *curr;
            if ai.ai_family == libc::AF_INET && !ai.ai_addr.is_null() {
                let sockaddr_in = *(ai.ai_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(sockaddr_in.sin_addr.s_addr));
                addrs.push(SocketAddr::new(IpAddr::V4(ip), 443));
            }
            curr = ai.ai_next;
        }
    }
    unsafe { libc::freeaddrinfo(res); }
    addrs
}

/// Tier 3: DoH over HTTPS via Google (https://8.8.8.8/resolve?name=...)
async fn doh_google_lookup(host: &str) -> Vec<IpAddr> {
    let url = format!("https://8.8.8.8/resolve?name={host}&type=A");
    let client = match reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let response = match client.get(&url).header("Host", "dns.google").send().await {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let json: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut ips = Vec::new();
    if let Some(answers) = json["Answer"].as_array() {
        for ans in answers {
            if ans["type"].as_u64() == Some(1) {
                if let Some(ip_str) = ans["data"].as_str() {
                    if let Ok(ip) = ip_str.parse::<Ipv4Addr>() {
                        ips.push(IpAddr::V4(ip));
                    }
                }
            }
        }
    }
    ips
}

/// Tier 4: Fallback known IPs for Qobuz CDN / API endpoints
fn fallback_known_ips(_host: &str) -> Vec<SocketAddr> {
    let defaults = ["23.215.212.238", "23.215.212.230", "23.211.14.66", "23.211.14.65"];
    let mut addrs = Vec::new();
    for ip_str in defaults {
        if let Ok(ip) = ip_str.parse::<Ipv4Addr>() {
            addrs.push(SocketAddr::new(IpAddr::V4(ip), 443));
        }
    }
    addrs
}

#[derive(Debug, Default)]
pub struct QbzDnsResolver;

impl Resolve for QbzDnsResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host_str = name.as_str().to_string();

            // Tier 1: C getaddrinfo (IPv4)
            let addrs = getaddrinfo_ipv4(&host_str);
            if !addrs.is_empty() {
                log::info!("[QBZ DNS] Tier 1 C getaddrinfo resolved {} to {:?}", host_str, addrs);
                return Ok(Box::new(addrs.into_iter()) as Box<dyn Iterator<Item = SocketAddr> + Send>);
            }

            // Tier 2: Tokio lookup_host
            let host_port = format!("{}:443", host_str);
            if let Ok(tokio_addrs) = tokio::net::lookup_host(&host_port).await {
                let vec: Vec<SocketAddr> = tokio_addrs.collect();
                if !vec.is_empty() {
                    log::info!("[QBZ DNS] Tier 2 Tokio lookup_host resolved {} to {:?}", host_str, vec);
                    return Ok(Box::new(vec.into_iter()) as Box<dyn Iterator<Item = SocketAddr> + Send>);
                }
            }

            log::warn!("[QBZ DNS] System DNS failed for {host_str}, attempting Tier 3 DoH lookup...");

            // Tier 3: Google DoH over HTTPS (direct IP 8.8.8.8)
            let doh_ips = doh_google_lookup(&host_str).await;
            if !doh_ips.is_empty() {
                let addrs: Vec<SocketAddr> = doh_ips.into_iter().map(|ip| SocketAddr::new(ip, 443)).collect();
                log::info!("[QBZ DNS] Tier 3 DoH resolved {} to {:?}", host_str, addrs);
                return Ok(Box::new(addrs.into_iter()) as Box<dyn Iterator<Item = SocketAddr> + Send>);
            }

            // Tier 4: Fallback known Akamai CDN IPs
            log::warn!("[QBZ DNS] DoH failed for {host_str}, using Tier 4 fallback IPs...");
            let fallback = fallback_known_ips(&host_str);
            Ok(Box::new(fallback.into_iter()) as Box<dyn Iterator<Item = SocketAddr> + Send>)
        })
    }
}
