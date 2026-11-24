FROM rust:1-bookworm AS build
WORKDIR /app
COPY Cargo.toml ./
COPY src/ src/
RUN cargo build --release
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates ffmpeg openssl python3 python3-numpy python3-pandas python3-matplotlib iproute2 && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/moqstreamlab /usr/local/bin/moqstreamlab
WORKDIR /lab
COPY lab.py analyze.py ./
CMD ["python3","lab.py","--binary","/usr/local/bin/moqstreamlab"]
