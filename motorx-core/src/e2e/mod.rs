use std::{fs, sync::Arc, time::Duration};

use bytes::Bytes;
use http::{
    header::{CONNECTION, UPGRADE},
    Request, Response, StatusCode, Version,
};
use http_body_util::{BodyExt, Empty, Full};
use hyper::client;
use hyper_util::rt::TokioIo;
use maplit::hashmap;
use tokio::{io::AsyncReadExt, join};
use utils::{start_rule, CertKeyFiles, TestUpstream};

use crate::{config::Tls, tcp_connect, Config, Server};

mod utils;

#[tokio::test]
async fn simple() {
    utils::tracing();

    let mut upstream =
        TestUpstream::new(
            |_| async move { Response::builder().body(Empty::new().boxed()).unwrap() },
        )
        .await;

    let config = Config {
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_uri = format!("http://{}", server.local_addr(0).unwrap());
    tokio::spawn(async move {
        server.run().await.unwrap();
        println!("server task eneded!!");
    });
    let client = utils::client();

    let _ = client.get(server_uri).send().await.unwrap();

    assert_eq!(upstream.requests_received().await.len(), 1);
}

#[tokio::test]
async fn simple_http2() {
    utils::tracing();

    let mut upstream =
        TestUpstream::new(
            |_| async move { Response::builder().body(Empty::new().boxed()).unwrap() },
        )
        .await;

    let config = Config {
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_uri = format!("http://{}", server.local_addr(0).unwrap());
    tokio::spawn(async move {
        server.run().await.unwrap();
        println!("server task eneded!!");
    });
    let client = utils::http2_client();

    let _ = client.get(server_uri).send().await.unwrap();

    assert_eq!(upstream.requests_received().await.len(), 1);
}

#[tokio::test]
async fn simple_tls() {
    utils::tracing();
    let CertKeyFiles {
        cert_file,
        key_file,
    } = utils::gen_self_signed();

    let mut upstream =
        TestUpstream::new(
            |_| async move { Response::builder().body(Empty::new().boxed()).unwrap() },
        )
        .await;

    let config = Config {
        tls: Some(Tls::File {
            certs: cert_file.path().to_str().unwrap().into(),
            private_key: key_file.path().to_str().unwrap().into(),
        }),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_uri = format!("https://localhost:{}", server.local_addr(0).unwrap().port());
    tokio::spawn(async move {
        server.run().await.unwrap();
    });
    let client = utils::file_tls_client(fs::read_to_string(cert_file.path()).unwrap());

    let _ = client.get(server_uri).send().await.unwrap();

    assert_eq!(upstream.requests_received().await.len(), 1);
}

#[tokio::test]
async fn simple_tls_http2() {
    utils::tracing();
    let CertKeyFiles {
        cert_file,
        key_file,
    } = utils::gen_self_signed();

    let mut upstream =
        TestUpstream::new(
            |_| async move { Response::builder().body(Empty::new().boxed()).unwrap() },
        )
        .await;

    let config = Config {
        tls: Some(Tls::File {
            certs: cert_file.path().to_str().unwrap().into(),
            private_key: key_file.path().to_str().unwrap().into(),
        }),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_uri = format!("https://localhost:{}", server.local_addr(0).unwrap().port());
    tokio::spawn(async move {
        server.run().await.unwrap();
    });
    let client = utils::http2_file_tls_client(fs::read_to_string(cert_file.path()).unwrap());

    let _ = client.get(server_uri).send().await.unwrap();

    assert_eq!(upstream.requests_received().await.len(), 1);
}

// TODO: find a way to test acme automatically
#[allow(unused)]
async fn simple_tls_acme() {
    utils::tracing();

    let mut upstream =
        TestUpstream::new(
            |_| async move { Response::builder().body(Empty::new().boxed()).unwrap() },
        )
        .await;

    let temp_dir = tempfile::tempdir().unwrap();

    let config = Config {
        tls: Some(Tls::Acme {
            domains: Arc::from(["localhost".to_string()]),
            cache_dir: temp_dir.path().to_path_buf(),
        }),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_uri = format!("https://localhost:{}", server.local_addr(0).unwrap().port());
    tokio::spawn(async move {
        server.run().await.unwrap();
    });
    let client = utils::client();

    let _ = client.get(server_uri).send().await.unwrap();

    assert_eq!(upstream.requests_received().await.len(), 1);
}

#[tokio::test]
async fn remove_match() {
    utils::tracing();

    let mut upstream =
        TestUpstream::new(
            |_| async move { Response::builder().body(Empty::new().boxed()).unwrap() },
        )
        .await;

    let config = Config {
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/service", &upstream, true)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_uri = format!("http://{}/service", server.local_addr(0).unwrap());
    tokio::spawn(async move {
        server.run().await.unwrap();
        println!("server task eneded!!");
    });
    let client = utils::client();

    let _ = client.get(server_uri).send().await.unwrap();

    let reqs = upstream.requests_received().await;
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    assert_eq!(req.uri().path(), "/");
}

// TODO: make better upgrade test
#[tokio::test]
async fn upgrade() {
    utils::tracing();

    let mut upstream = TestUpstream::new(|_| async move {
        Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .body(Empty::new().boxed())
            .unwrap()
    })
    .await;

    let config = Config {
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_addr = server.local_addr(0).unwrap();
    tokio::spawn(async move {
        server.run().await.unwrap();
    });
    let stream = tcp_connect(server_addr).await.unwrap();
    let (mut sender, conn) = client::conn::http1::Builder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .handshake::<_, Empty<Bytes>>(TokioIo::new(stream))
        .await
        .unwrap();

    tokio::spawn(async move {
        if let Err(err) = conn.with_upgrades().await {
            eprintln!("conn err: {err:?}");
        }
    });

    let req = Request::builder()
        .header(CONNECTION, "upgrade")
        .header(UPGRADE, "foo")
        .body(Empty::<Bytes>::new())
        .unwrap();

    let res = sender.send_request(req).await.unwrap();

    assert_eq!(res.status(), StatusCode::SWITCHING_PROTOCOLS);

    let upgraded = hyper::upgrade::on(res).await.unwrap();
    let mut conn = TokioIo::new(upgraded);
    let mut buf = vec![0; 1024];
    let num_read = conn.read(&mut buf).await;
    assert!(num_read.unwrap() != 0);

    assert_eq!(upstream.requests_received().await.len(), 1);
}

#[tokio::test]
async fn h2_upstream() {
    utils::tracing();

    let mut upstream =
        TestUpstream::new(
            |_| async move { Response::builder().body(Empty::new().boxed()).unwrap() },
        )
        .await;

    let config = Config {
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_h2_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_uri = format!("http://{}", server.local_addr(0).unwrap());
    tokio::spawn(async move {
        server.run().await.unwrap();
    });
    let client = utils::client();

    // Parallel requests, but only one connection
    let (r1, r2, r3) = join!(
        client.get(&server_uri).send(),
        client.get(&server_uri).send(),
        client.get(&server_uri).send()
    );

    r1.unwrap();
    r2.unwrap();
    r3.unwrap();

    assert_eq!(upstream.requests_received().await.len(), 3);
    assert_eq!(upstream.connections_accepted(), 1);
}

#[tokio::test]
async fn simple_h3_http1_upstream() {
    utils::tracing();
    let CertKeyFiles {
        cert_file,
        key_file,
    } = utils::gen_self_signed();

    let mut upstream = TestUpstream::new(|_| async move {
        Response::builder()
            .body(Full::new(Bytes::from_static(b"hi from upstream")).boxed())
            .unwrap()
    })
    .await;

    let config = Config {
        tls: Some(Tls::File {
            certs: cert_file.path().to_str().unwrap().into(),
            private_key: key_file.path().to_str().unwrap().into(),
        }),
        h3_addr: Some("[::1]:0".parse().unwrap()),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let h3_port = server.h3_local_addr().unwrap().unwrap().port();
    let server_uri = format!("https://localhost:{}", h3_port);
    tokio::spawn(async move {
        server.run().await.unwrap();
    });
    let client = utils::h3_file_tls_client(fs::read_to_string(cert_file.path()).unwrap(), h3_port);

    let res = client
        .get(server_uri)
        .version(Version::HTTP_3)
        .body("hello h3!")
        .send()
        .await
        .unwrap();
    assert_eq!(&*res.bytes().await.unwrap(), b"hi from upstream");

    let upstream_req = &upstream.requests_received().await[0];
    assert_eq!(upstream_req.body(), "hello h3!");
}

#[tokio::test]
async fn simple_h3_h2_upstream() {
    utils::tracing();
    let CertKeyFiles {
        cert_file,
        key_file,
    } = utils::gen_self_signed();

    let mut upstream = TestUpstream::new(|_| async move {
        Response::builder()
            .body(Full::new(Bytes::from_static(b"hi from upstream")).boxed())
            .unwrap()
    })
    .await;

    let config = Config {
        tls: Some(Tls::File {
            certs: cert_file.path().to_str().unwrap().into(),
            private_key: key_file.path().to_str().unwrap().into(),
        }),
        h3_addr: Some("[::1]:0".parse().unwrap()),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_h2_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let h3_port = server.h3_local_addr().unwrap().unwrap().port();
    let server_uri = format!("https://localhost:{}", h3_port);
    tokio::spawn(async move {
        server.run().await.unwrap();
    });
    let client = utils::h3_file_tls_client(fs::read_to_string(cert_file.path()).unwrap(), h3_port);

    let res = client
        .get(server_uri)
        .version(Version::HTTP_3)
        .body("hello h3!")
        .send()
        .await
        .unwrap();
    assert_eq!(&*res.bytes().await.unwrap(), b"hi from upstream");

    let upstream_req = &upstream.requests_received().await[0];
    assert_eq!(upstream_req.body(), "hello h3!");
}

#[tokio::test]
async fn multi_listener_proxy_protocol() {
    use tokio::io::AsyncWriteExt;

    use crate::config::TcpAddr;

    utils::tracing();

    let mut upstream =
        TestUpstream::new(
            |_| async move { Response::builder().body(Empty::new().boxed()).unwrap() },
        )
        .await;

    let config = Config {
        tcp_addrs: vec![
            "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap().into(),
            TcpAddr {
                addr: "127.0.0.1:0".parse().unwrap(),
                proxy_protocol: true,
            },
        ],
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let plain_addr = server.local_addr(0).unwrap();
    let pp_addr = server.local_addr(1).unwrap();
    tokio::spawn(async move {
        server.run().await.unwrap();
    });

    // A connection with an invalid PROXY header must not stall the listener
    let mut bad = tcp_connect(pp_addr).await.unwrap();
    bad.write_all(b"definitely not a proxy protocol header")
        .await
        .unwrap();

    // Neither must a connection that never sends its header
    let _idle = tcp_connect(pp_addr).await.unwrap();

    let mut stream = tcp_connect(pp_addr).await.unwrap();
    let mut header = Vec::new();
    header.extend_from_slice(&[
        0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A,
    ]);
    header.push(0x21); // v2, PROXY
    header.push(0x11); // AF_INET, STREAM
    header.extend_from_slice(&12u16.to_be_bytes());
    header.extend_from_slice(&[1, 2, 3, 4]); // src addr
    header.extend_from_slice(&[5, 6, 7, 8]); // dst addr
    header.extend_from_slice(&9999u16.to_be_bytes()); // src port
    header.extend_from_slice(&80u16.to_be_bytes()); // dst port
    stream.write_all(&header).await.unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n")
        .await
        .unwrap();

    let mut buf = [0u8; 1024];
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert!(buf[..n].starts_with(b"HTTP/1.1 200"));

    // The plain listener still works alongside
    let client = utils::client();
    let _ = client
        .get(format!("http://{plain_addr}"))
        .send()
        .await
        .unwrap();

    let requests = upstream.requests_received().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].headers().get("x-forwarded-for").unwrap(),
        "1.2.3.4:9999"
    );
}
