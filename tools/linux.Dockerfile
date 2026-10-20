# The Linux build: one static x86_64 binary (musl), so it runs on any distribution, the Steam Deck included,
# with nothing to install beside it. Also what the Linux tests run in.
#
#   docker build -t starrailsim-sync-linux -f tools/linux.Dockerfile tools
#   docker run --rm --cpus 2 -v <repo>:/src:ro -v srs-cargo:/usr/local/cargo/registry -v srs-git:/usr/local/cargo/git \
#     -v srs-target:/target -e CARGO_TARGET_DIR=/target -w /src starrailsim-sync-linux \
#     cargo build --locked --release --target x86_64-unknown-linux-musl
FROM rust:1.97-bookworm
RUN apt-get update \
 && apt-get install -y --no-install-recommends musl-tools python3 iproute2 libcap2-bin procps sudo socat \
 && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl
