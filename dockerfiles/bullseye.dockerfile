ARG RUST_IMAGE=rust:slim
FROM $RUST_IMAGE as build

COPY Cargo.toml Cargo.toml
COPY src/ src/
COPY motorx-core/ motorx-core

RUN cargo build --release

RUN cp target/release/motorx /motorx

FROM debian:bullseye-slim

COPY --from=build /motorx /motorx
COPY motorx.json /motorx.json

ENTRYPOINT [ "/motorx" ]
