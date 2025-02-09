use std::{fs, sync::Arc};

use bytes::Bytes;
use http::{
    header::{CONNECTION, UPGRADE},
    Request, Response, StatusCode,
};
use http_body_util::{BodyExt, Empty};
use hyper::client;
use hyper_util::rt::TokioIo;
use maplit::hashmap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use utils::{start_rule, CertKeyFiles, TestUpstream};

use crate::{config::Tls, tcp_connect, Config, Server};

mod utils;

#[tokio::test]
async fn simple() {
    utils::tracing();

    let mut upstream = TestUpstream::new_http1(|_| async move {
        Response::builder().body(Empty::new().boxed()).unwrap()
    })
    .await;

    let config = Config {
        addr: "127.0.0.1:0".parse().unwrap(),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_uri = format!("http://{}", server.local_addr().unwrap());
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

    let mut upstream = TestUpstream::new_http1(|_| async move {
        Response::builder().body(Empty::new().boxed()).unwrap()
    })
    .await;

    let config = Config {
        addr: "127.0.0.1:0".parse().unwrap(),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_uri = format!("http://{}", server.local_addr().unwrap());
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

    let mut upstream = TestUpstream::new_http1(|_| async move {
        Response::builder().body(Empty::new().boxed()).unwrap()
    })
    .await;

    let config = Config {
        tls: Some(Tls::File {
            certs: cert_file.path().to_str().unwrap().into(),
            private_key: key_file.path().to_str().unwrap().into(),
        }),
        addr: "127.0.0.1:0".parse().unwrap(),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_uri = format!("https://localhost:{}", server.local_addr().unwrap().port());
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

    let mut upstream = TestUpstream::new_http1(|_| async move {
        Response::builder().body(Empty::new().boxed()).unwrap()
    })
    .await;

    let config = Config {
        tls: Some(Tls::File {
            certs: cert_file.path().to_str().unwrap().into(),
            private_key: key_file.path().to_str().unwrap().into(),
        }),
        addr: "127.0.0.1:0".parse().unwrap(),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_uri = format!("https://localhost:{}", server.local_addr().unwrap().port());
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

    let mut upstream = TestUpstream::new_http1(|_| async move {
        Response::builder().body(Empty::new().boxed()).unwrap()
    })
    .await;

    let temp_dir = tempfile::tempdir().unwrap();

    let config = Config {
        tls: Some(Tls::Acme {
            domains: Arc::from(["localhost".to_string()]),
            cache_dir: temp_dir.path().to_path_buf(),
        }),
        addr: "127.0.0.1:0".parse().unwrap(),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_uri = format!("https://localhost:{}", server.local_addr().unwrap().port());
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

    let mut upstream = TestUpstream::new_http1(|_| async move {
        Response::builder().body(Empty::new().boxed()).unwrap()
    })
    .await;

    let config = Config {
        addr: "127.0.0.1:0".parse().unwrap(),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/service", &upstream, true)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_uri = format!("http://{}/service", server.local_addr().unwrap());
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

    let mut upstream = TestUpstream::new_http1(|_| async move {
        Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .body(Empty::new().boxed())
            .unwrap()
    })
    .await;

    let config = Config {
        addr: "127.0.0.1:0".parse().unwrap(),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream, false)],
        ..Default::default()
    };
    let server = Server::new(config).unwrap();
    let server_addr = server.local_addr().unwrap();
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
    conn.write_all(b"hi there!").await.unwrap();
    let mut buf = vec![0; 128];
    let num_read = conn.read(&mut buf).await.unwrap();
    assert!(num_read != 0);

    assert_eq!(upstream.requests_received().await.len(), 1);
}
