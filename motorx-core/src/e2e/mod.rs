use std::{collections::HashMap, fs};

use http::Response;
use http_body_util::{BodyExt, Empty};
use maplit::hashmap;
use utils::{start_rule, CertKeyFiles, TestUpstream};

use crate::{config::match_type::MatchType, Config, Rule, Server};

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
        rules: vec![start_rule("/", &upstream)],
        ..Default::default()
    };
    let server = Server::new(config);
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
        rules: vec![start_rule("/", &upstream)],
        ..Default::default()
    };
    let server = Server::new(config);
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
        certs: Some(cert_file.path().to_str().unwrap().into()),
        private_key: Some(key_file.path().to_str().unwrap().into()),
        addr: "127.0.0.1:0".parse().unwrap(),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream)],
        ..Default::default()
    };
    let server = Server::new_tls(config);
    let server_uri = format!("https://localhost:{}", server.local_addr().unwrap().port());
    tokio::spawn(async move {
        server.run().await.unwrap();
    });
    let client = utils::tls_client(fs::read_to_string(cert_file.path()).unwrap());

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
        certs: Some(cert_file.path().to_str().unwrap().into()),
        private_key: Some(key_file.path().to_str().unwrap().into()),
        addr: "127.0.0.1:0".parse().unwrap(),
        upstreams: hashmap! {
            upstream.id().to_string() => upstream.as_upstream()
        },
        rules: vec![start_rule("/", &upstream)],
        ..Default::default()
    };
    let server = Server::new_tls(config);
    let server_uri = format!("https://localhost:{}", server.local_addr().unwrap().port());
    tokio::spawn(async move {
        server.run().await.unwrap();
    });
    let client = utils::http2_tls_client(fs::read_to_string(cert_file.path()).unwrap());

    let _ = client.get(server_uri).send().await.unwrap();

    assert_eq!(upstream.requests_received().await.len(), 1);
}
