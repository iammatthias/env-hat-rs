# Local CI image: arm64 Debian trixie, the same platform and glibc as the Pi.
# Built by ci/run.sh; nothing here talks to a hosted CI service.
FROM --platform=linux/arm64 debian:trixie
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl build-essential pkg-config \
 && rm -rf /var/lib/apt/lists/*
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable --component rustfmt,clippy
ENV PATH=/root/.cargo/bin:$PATH
RUN cargo install cargo-audit --locked
WORKDIR /work
