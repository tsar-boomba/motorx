use std::{
    fmt::{Debug, Write},
    sync::Arc,
};

use bytes::BytesMut;
use http::{Request, Response};
use prometheus_client::{
    encoding::{text::encode, EncodeLabelSet, EncodeLabelValue},
    metrics::{counter::Counter, family::Family},
    registry::Registry,
};

#[derive(Debug, Clone)]
pub struct StatsCollector {
    registry: Arc<Registry>,
    requests: Family<ReqLabels, Counter>,
    responses: Family<ResLabels, Counter>,
    connections: Family<ConnLabels, Counter>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ReqLabels {
    method: Method,
    path: SharedStr,
    version: Version,
	host: Option<SharedStr>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ResLabels {
    status: StatusCode,
    version: Version,
	path: SharedStr,
	host: Option<SharedStr>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ConnLabels {
    conn_type: ConnType,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct SharedStr(Arc<str>);

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Method(http::Method);

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Version(http::Version);

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct StatusCode(http::StatusCode);

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
enum ConnType {
    Tcp,
    Quic,
}

impl StatsCollector {
    pub fn new() -> Self {
        let mut registry = <Registry>::with_prefix("motorx");

        let requests = Family::<ReqLabels, Counter>::default();
        let responses = Family::<ResLabels, Counter>::default();
        let connections = Family::<ConnLabels, Counter>::default();

        registry.register(
            "http_requests",
            "Number of HTTP requests received",
            requests.clone(),
        );

        registry.register(
            "http_responses",
            "Number of HTTP responses sent",
            responses.clone(),
        );

        registry.register(
            "connections",
            "number of connections accepted",
            connections.clone(),
        );

        Self {
            registry: Arc::new(registry),
            requests,
            responses,
            connections,
        }
    }

    pub fn add_req<B>(&self, req: &Request<B>, path: Arc<str>, host: Option<Arc<str>>) {
        // TODO: consider interning path especially
        self.requests
            .get_or_create(&ReqLabels {
                method: Method(req.method().clone()),
                path: SharedStr(path),
                version: Version(req.version()),
				host: host.map(SharedStr)
            })
            .inc();
    }

    pub fn add_res<B>(&self, res: &Response<B>, path: Arc<str>, host: Option<Arc<str>>) {
        self.responses
            .get_or_create(&ResLabels {
                status: StatusCode(res.status()),
                version: Version(res.version()),
				path: SharedStr(path),
				host: host.map(SharedStr)
            })
            .inc();
    }

    pub fn add_quic_conn(&self) {
        self.connections
            .get_or_create(&ConnLabels {
                conn_type: ConnType::Quic,
            })
            .inc();
    }

    pub fn add_tcp_conn(&self) {
        self.connections
            .get_or_create(&ConnLabels {
                conn_type: ConnType::Tcp,
            })
            .inc();
    }

    pub fn encode(&self, to: &mut BytesMut) -> Result<(), std::fmt::Error> {
		encode(to, &self.registry)
    }
}

impl EncodeLabelValue for SharedStr {
	fn encode(&self, encoder: &mut prometheus_client::encoding::LabelValueEncoder) -> Result<(), std::fmt::Error> {
		encoder.write_str(&self.0)
	}
}

impl EncodeLabelValue for Method {
    fn encode(
        &self,
        encoder: &mut prometheus_client::encoding::LabelValueEncoder,
    ) -> Result<(), std::fmt::Error> {
        encoder.write_str(self.0.as_str())
    }
}

impl EncodeLabelValue for Version {
    fn encode(
        &self,
        encoder: &mut prometheus_client::encoding::LabelValueEncoder,
    ) -> Result<(), std::fmt::Error> {
        write!(encoder, "{:?}", self.0)
    }
}

impl EncodeLabelValue for StatusCode {
    fn encode(
        &self,
        encoder: &mut prometheus_client::encoding::LabelValueEncoder,
    ) -> Result<(), std::fmt::Error> {
        write!(encoder, "{}", self.0.as_u16())
    }
}
