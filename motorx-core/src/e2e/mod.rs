use std::{collections::HashMap, fs};

use http::Response;
use http_body_util::{BodyExt, Empty};
use tracing_subscriber::EnvFilter;
use utils::TestUpstream;

use crate::{config::match_type::MatchType, Config, Rule, Server};

mod utils;

#[tokio::test]
async fn simple() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let mut upstreams = HashMap::new();
    let mut upstream = TestUpstream::new_http1(|_| async move {
        Response::builder().body(Empty::new().boxed()).unwrap()
    })
    .await;

    upstreams.insert(upstream.id().to_string(), upstream.as_upstream());

    let config = Config {
        addr: "127.0.0.1:0".parse().unwrap(),
        upstreams,
        rules: vec![Rule {
            cache: None,
            match_headers: None,
            path: MatchType::Start("/".into()),
            upstream: upstream.id().to_string(),
            cache_key: 0,
            upstream_key: 0,
        }],
        ..Default::default()
    };
    let server = Server::new(config);
    let server_uri = format!("http://{}", server.local_addr().unwrap());
    tokio::spawn(async move {
        server.run().await.unwrap();
        println!("server task eneded!!");
    });
    let client = utils::client();

    println!("Sent request");
    let res = client.get(server_uri).send().await.unwrap();
    println!("Got res: {res:?}");

    assert_eq!(upstream.requests_received().await.len(), 1);
}

#[tokio::test]
async fn simple_tls() {
    let (cert_file, key_file) = utils::gen_self_signed();
    let mut upstreams = HashMap::new();
    let mut upstream = TestUpstream::new_http1(|_| async move {
        Response::builder().body(Empty::new().boxed()).unwrap()
    })
    .await;

    upstreams.insert(upstream.id().to_string(), upstream.as_upstream());

    let config = Config {
        certs: Some(cert_file.path().to_str().unwrap().into()),
        private_key: Some(key_file.path().to_str().unwrap().into()),
        addr: "127.0.0.1:0".parse().unwrap(),
        upstreams,
        rules: vec![Rule {
            cache: None,
            match_headers: None,
            path: MatchType::Start("/".into()),
            upstream: upstream.id().to_string(),
            cache_key: 0,
            upstream_key: 0,
        }],
        ..Default::default()
    };
    let server = Server::new_tls(config);
    let server_uri = format!("https://localhost:{}", server.local_addr().unwrap().port());
    tokio::spawn(async move {
        server.run().await.unwrap();
        println!("server task eneded!!");
    });
    let client = utils::tls_client(fs::read_to_string(cert_file.path()).unwrap());

    println!("Sent request");
    let res = client.get(server_uri).send().await.unwrap();
    println!("Got res: {res:?}");

    assert_eq!(upstream.requests_received().await.len(), 1);
}
