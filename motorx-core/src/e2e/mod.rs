use std::{collections::HashMap, time::Duration};

use http::Response;
use http_body_util::{BodyExt, Empty};
use tracing_subscriber::EnvFilter;
use utils::TestUpstream;

use crate::{config::match_type::MatchType, Config, Rule, Server};

mod utils;

#[tokio::test]
async fn test() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let mut upstreams = HashMap::new();
    println!("create upstream");
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
        }],
        ..Default::default()
    };
    let server = Server::new(config);
    let server_uri = format!("http://{}", server.local_addr().unwrap());
    tokio::spawn(async move {
        server.run().await.unwrap();
        println!("server task eneded!!");
    });
    let client = client();

    println!("Sent request");
    let res = client.get(server_uri).send().await.unwrap();
    println!("Got res: {res:?}");

    assert_eq!(upstream.requests_received().await.len(), 1);
}

fn client() -> reqwest::Client {
    reqwest::ClientBuilder::new()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap()
}
