//! Public podcast HTTP only. Do not use for Iroh or the private audio server.
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

fn public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    // IANA special-purpose ranges, multicast and future/reserved space.
    // Protocol anycast exceptions inside these ranges are not podcast origins.
    !matches!(a, 0 | 10 | 127 | 224..=255)
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 169 && b == 254)
        && !(a == 172 && (16..=31).contains(&b))
        && !(a == 192 && ((b == 0 && matches!(c, 0 | 2)) || (b == 88 && c == 99) || b == 168))
        && !(a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
        && !(a == 203 && b == 0 && c == 113)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_ipv4(ip),
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                return public_ipv4(ip);
            }
            let s = ip.segments();
            // Keep standard DNS64/NAT64 mobile networks working, but validate
            // their embedded IPv4 destination too. Local-use NAT64 is denied.
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let b = ip.octets();
                return public_ipv4(Ipv4Addr::new(b[12], b[13], b[14], b[15]));
            }
            // Ordinary global unicast only; exclude protocol assignments,
            // documentation and 6to4 (which can embed a private IPv4 address).
            s[0] & 0xe000 == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

fn public_hostname(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    !host.is_empty()
        && host != "localhost"
        && !host.ends_with(".localhost")
        && !host.ends_with(".local")
}

/// Syntax/literal check only. DNS names must also pass PublicResolver at connect time.
pub(crate) fn safe_public_https_url(url: &reqwest::Url) -> bool {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    if let Some(domain) = url.domain() {
        return public_hostname(domain);
    }
    // URL host_str serializes IPv6 with brackets; a parse failure must fail closed.
    url.host_str()
        .and_then(|host| {
            host.trim_start_matches('[')
                .trim_end_matches(']')
                .parse()
                .ok()
        })
        .is_some_and(public_ip)
}

fn checked_addresses(addresses: impl Iterator<Item = SocketAddr>) -> io::Result<Addrs> {
    let addresses: Vec<_> = addresses.collect();
    if addresses.is_empty() || addresses.iter().any(|address| !public_ip(address.ip())) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "podcast host must resolve only to public addresses",
        ));
    }
    // Give the connector the exact validated addresses. It must not resolve the
    // hostname again after validation (including on redirects/reconnections).
    Ok(Box::new(addresses.into_iter()))
}

struct SystemResolver;

impl Resolve for SystemResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            // Use the OS resolver, not Hickory's Android/JNI configuration path.
            let addresses = tokio::time::timeout(
                Duration::from_secs(8),
                tokio::net::lookup_host((name.as_str(), 0)),
            )
            .await??;
            Ok(Box::new(addresses.collect::<Vec<_>>().into_iter()) as Addrs)
        })
    }
}

struct PublicResolver<R>(Arc<R>);

impl<R: Resolve + 'static> Resolve for PublicResolver<R> {
    fn resolve(&self, name: Name) -> Resolving {
        let resolver = self.0.clone();
        Box::pin(async move {
            if !public_hostname(name.as_str()) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "private podcast hostname",
                )
                .into());
            }
            let addresses = resolver.resolve(name).await?;
            Ok(checked_addresses(addresses)?)
        })
    }
}

pub(crate) fn podcast_http_client(
    request_timeout: Duration,
    read_timeout: Duration,
) -> Result<reqwest::Client, String> {
    client_builder(request_timeout, read_timeout)
        .build()
        .map_err(|error| error.to_string())
}

fn client_builder(request_timeout: Duration, read_timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .https_only(true)
        // Proxies can resolve destinations themselves and bypass our resolver.
        .no_proxy()
        .no_hickory_dns()
        .dns_resolver(Arc::new(PublicResolver(Arc::new(SystemResolver))))
        .connect_timeout(Duration::from_secs(8))
        .read_timeout(read_timeout)
        .timeout(request_timeout)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || !safe_public_https_url(attempt.url()) {
                attempt.error("podcast redirect must target public HTTPS")
            } else {
                attempt.follow()
            }
        }))
        .user_agent("Napstrfy/0.1 (https://napstr.net)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_private_special_and_disguised_literal_addresses() {
        for host in [
            "localhost",
            "LOCALHOST.",
            "host.local",
            "foo.localhost",
            "127.0.0.1",
            "127.1",
            "2130706433",
            "0x7f000001",
            "0177.0.0.1",
            "0.0.0.0",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "100.64.0.1",
            "100.127.255.255",
            "169.254.169.254",
            "192.0.0.1",
            "192.0.2.1",
            "192.88.99.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "[::]",
            "[::1]",
            "[::127.0.0.1]",
            "[::ffff:127.0.0.1]",
            "[::ffff:c0a8:101]",
            "[fc00::1]",
            "[fd00::1]",
            "[fe80::1]",
            "[ff02::1]",
            "[2001:db8::1]",
            "[2001::1]",
            "[2002:7f00:1::1]",
            "[3fff::1]",
            "[64:ff9b::7f00:1]",
            "[64:ff9b:1::1]",
        ] {
            let url = reqwest::Url::parse(&format!("https://{host}/audio.mp3")).unwrap();
            assert!(!safe_public_https_url(&url), "allowed {url}");
        }
        for url in [
            "http://example.com/a",
            "https://user@example.com/a",
            "https://user:pass@example.com/a",
        ] {
            assert!(!safe_public_https_url(&reqwest::Url::parse(url).unwrap()));
        }
    }

    #[test]
    fn accepts_public_hosts_and_ipv4_ipv6_nat64_destinations() {
        for host in [
            "example.com",
            "cdn.example.com.",
            "8.8.8.8",
            "1.1.1.1",
            "100.128.0.1",
            "172.32.0.1",
            "[2606:4700:4700::1111]",
            "[2001:4860:4860::8888]",
            "[::ffff:8.8.8.8]",
            "[64:ff9b::808:808]",
        ] {
            assert!(
                safe_public_https_url(
                    &reqwest::Url::parse(&format!("https://{host}/audio.mp3")).unwrap()
                ),
                "rejected {host}"
            );
        }
    }

    #[test]
    fn validates_every_dns_answer_and_returns_the_exact_checked_addresses() {
        let public = [
            "8.8.8.8:0".parse().unwrap(),
            "[2606:4700:4700::1111]:0".parse().unwrap(),
        ];
        assert_eq!(
            checked_addresses(public.into_iter())
                .unwrap()
                .collect::<Vec<_>>(),
            public
        );
        assert!(checked_addresses(std::iter::empty()).is_err());
        for private in [
            "127.0.0.1:0",
            "192.168.1.1:0",
            "[::1]:0",
            "[::ffff:127.0.0.1]:0",
            "[fe80::1]:0",
        ] {
            // No safe subset may leak through before the entire answer is checked.
            assert!(checked_addresses([public[0], private.parse().unwrap()].into_iter()).is_err());
            assert!(checked_addresses([private.parse().unwrap(), public[0]].into_iter()).is_err());
        }
    }

    struct Answers {
        answers: std::sync::Mutex<Vec<Vec<SocketAddr>>>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl Resolve for Answers {
        fn resolve(&self, _: Name) -> Resolving {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let addresses = self.answers.lock().unwrap().remove(0);
            Box::pin(async move { Ok(Box::new(addresses.into_iter()) as Addrs) })
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        let _ = rustls::crypto::ring::default_provider().install_default();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn changing_dns_answers_are_revalidated_without_a_second_lookup() {
        runtime().block_on(async {
            let public: SocketAddr = "8.8.8.8:0".parse().unwrap();
            let source = Arc::new(Answers {
                answers: std::sync::Mutex::new(vec![
                    vec![public],
                    vec!["127.0.0.1:0".parse().unwrap()],
                ]),
                calls: std::sync::atomic::AtomicUsize::new(0),
            });
            let resolver = PublicResolver(source.clone());
            let checked = resolver
                .resolve("rebind.example".parse().unwrap())
                .await
                .unwrap();
            assert_eq!(source.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(checked.collect::<Vec<_>>(), vec![public]);
            assert!(resolver
                .resolve("rebind.example".parse().unwrap())
                .await
                .is_err());
            assert_eq!(source.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        });
    }

    #[test]
    fn connector_rejects_private_dns_before_opening_a_socket() {
        runtime().block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let source = Arc::new(Answers {
                answers: std::sync::Mutex::new(vec![vec![listener.local_addr().unwrap()]]),
                calls: std::sync::atomic::AtomicUsize::new(0),
            });
            let client = client_builder(Duration::from_secs(2), Duration::from_secs(2))
                .dns_resolver(Arc::new(PublicResolver(source.clone())))
                .build()
                .unwrap();
            assert!(client
                .get(format!(
                    "https://private.example:{}/",
                    listener.local_addr().unwrap().port()
                ))
                .send()
                .await
                .is_err());
            assert_eq!(source.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert!(
                tokio::time::timeout(Duration::from_millis(50), listener.accept())
                    .await
                    .is_err()
            );
        });
    }

    #[test]
    fn environment_proxy_cannot_bypass_public_resolver() {
        // Isolate environment variables in a child rather than racing parallel tests.
        const CHILD: &str = "NAPSTRFY_PROXY_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            connector_rejects_private_dns_before_opening_a_socket();
            return;
        }
        let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        proxy.set_nonblocking(true).unwrap();
        let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "public_http::tests::environment_proxy_cannot_bypass_public_resolver",
            ])
            .env(CHILD, "1")
            .env("HTTPS_PROXY", &proxy_url)
            .env("https_proxy", &proxy_url)
            .env("ALL_PROXY", &proxy_url)
            .env("all_proxy", &proxy_url)
            .env("NO_PROXY", "")
            .env("no_proxy", "")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert_eq!(
            proxy.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn redirects_cannot_reach_private_literals_or_private_dns_answers() {
        runtime().block_on(async {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            for host in ["127.0.0.1", "[::1]", "private.example"] {
                let origin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let origin_addr = origin.local_addr().unwrap();
                let location = format!("https://{host}:{}/secret", target.local_addr().unwrap().port());
                let server = tokio::spawn(async move {
                    let (mut socket, _) = origin.accept().await.unwrap();
                    let mut buffer = [0; 4096];
                    socket.read(&mut buffer).await.unwrap();
                    socket.write_all(format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
                });
                let source = Arc::new(Answers {
                    answers: std::sync::Mutex::new(vec![vec![target.local_addr().unwrap()]]),
                    calls: std::sync::atomic::AtomicUsize::new(0),
                });
                // Only the local test origin uses HTTP and a DNS override.
                // Production always requires HTTPS and has no address overrides.
                let client = client_builder(Duration::from_secs(2), Duration::from_secs(2))
                    .https_only(false)
                    .resolve("origin.example", origin_addr)
                    .dns_resolver(Arc::new(PublicResolver(source.clone())))
                    .build().unwrap();
                assert!(client.get(format!("http://origin.example:{}/", origin_addr.port())).send().await.is_err());
                server.await.unwrap();
                assert_eq!(source.calls.load(std::sync::atomic::Ordering::SeqCst), usize::from(host == "private.example"));
                assert!(tokio::time::timeout(Duration::from_millis(50), target.accept()).await.is_err());
            }
        });
    }
}
