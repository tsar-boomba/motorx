ARG RUST_IMAGE=rust:1.84-slim
FROM ${RUST_IMAGE} AS build

COPY Cargo.toml Cargo.toml
COPY src/ src/
COPY motorx-core/ motorx-core

RUN cargo build --release

RUN cp target/release/motorx /motorx

FROM debian:bookworm-slim

COPY --from=build /motorx /motorx

ENTRYPOINT [ "/motorx", "/motorx.json" ]
