FROM rust:1.92.0-bookworm@sha256:e90e846de4124376164ddfbaab4b0774c7bdeef5e738866295e5a90a34a307a2 AS base
RUN apt-get update -qq \
    && apt-get install -y --no-install-recommends util-linux \
    && rm -rf /var/lib/apt/lists/* \
    && rustup component add rustfmt clippy \
    && mkdir /out && chown nobody:nogroup /out
USER nobody:nogroup
SHELL ["setpriv", "--no-new-privs", "/bin/sh", "-eu", "-c"]
ENV CARGO_HOME=/out/cargo CARGO_TARGET_DIR=/out/target CARGO_INCREMENTAL=0 \
    GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory GIT_CONFIG_VALUE_0=/work
WORKDIR /work/src
ARG SOURCE_REVISION
ENV GITHUB_SHA=$SOURCE_REVISION

FROM base AS sdks
RUN --mount=type=bind,target=/work \
    cargo test -p skvoz-network-artifact --tests --release --locked \
    && cargo run -p skvoz-network-artifact --bin export-sdk --release --locked -- /work desktop-host /out/desktop-host \
    && cargo run -p skvoz-network-artifact --bin export-sdk --release --locked -- /work portable-supplier /out/portable-supplier \
    && mkdir /out/sdks \
    && cp /out/desktop-host/*.tar.gz /out/portable-supplier/*.tar.gz /out/sdks/

FROM scratch AS sdk-artifacts
COPY --from=sdks /out/sdks/ /

FROM base AS tools
RUN --mount=type=bind,target=/work cargo build -p skvoz-network-artifact --bin skvoz-network-artifact --bin core-release --release --locked

FROM tools AS linux
RUN --mount=type=bind,target=/work \
    /out/target/release/core-release --public fetch x86_64-unknown-linux-gnu /out/core
ENV SKVOZ_CORE_ARTIFACT=/out/core LD_LIBRARY_PATH=/out/core/lib
RUN --mount=type=bind,target=/work \
    cargo build -p skvoz-network --features linux-runtime -p skvoz-network-helper --release --locked \
    && mkdir /out/linux \
    && /work/src/tools/bundle-native.sh /out/target/release/skvoz-network-artifact /out/target/release /out/core/lib/libskvoz_core.so /out/linux/skvoz-network-0.7.0-x86_64-unknown-linux-gnu.tar.gz \
    && mkdir /out/native \
    && tar -xzf /out/linux/*.tar.gz -C /out/native --no-same-owner

# OS-only dependency preparation; Rust integration tests own every scenario.
FROM moby/buildkit@sha256:a82d1ab899cda51aade6fe818d71e4b58c4079e047a0cf29dbb93b2b0465ea69 AS trust-seed
FROM ubuntu:24.04@sha256:a853f94d226358a79c740cfc7bce0c289748f3fe3488d921d038ccd752c61b60 AS test-os
ARG UBUNTU_SNAPSHOT=20261019T000000Z
# The official immutable image fixes the trust bytes. Copy no BuildKit programs.
COPY --from=trust-seed /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
# Signed immutable package indices keep this fixture recipe independent of
# current noble updates. Products and NATS are separate verified inputs.
RUN rm -f /etc/apt/sources.list /etc/apt/sources.list.d/ubuntu.sources \
    && printf '%s\n' \
      "deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://snapshot.ubuntu.com/ubuntu/${UBUNTU_SNAPSHOT}/ noble main universe" \
      "deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://snapshot.ubuntu.com/ubuntu/${UBUNTU_SNAPSHOT}/ noble-updates main universe" \
      "deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://snapshot.ubuntu.com/ubuntu/${UBUNTU_SNAPSHOT}/ noble-security main universe" \
      > /etc/apt/sources.list \
    && apt-get -o Acquire::Check-Valid-Until=false update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      iproute2 nftables conntrack ca-certificates util-linux openssl \
      dbus polkitd systemd-resolved \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 skvoz \
    && useradd --uid 10001 --gid 10001 --no-create-home skvoz

# Ordinary language checks share Ubuntu with the kernel actors, while the
# candidate runtime/helper retain the independently compiled Bookworm ABI.
FROM test-os AS test-tools
RUN apt-get -o Acquire::Check-Valid-Until=false update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      build-essential git curl pkg-config \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir /out && chown nobody:nogroup /out
COPY --from=base /usr/local/rustup /usr/local/rustup
COPY --from=base /usr/local/cargo/bin /usr/local/cargo/bin
ENV PATH=/usr/local/cargo/bin:$PATH RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/out/cargo CARGO_TARGET_DIR=/out/target CARGO_INCREMENTAL=0 \
    GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory GIT_CONFIG_VALUE_0=/work
USER nobody:nogroup
SHELL ["setpriv", "--no-new-privs", "/bin/sh", "-eu", "-c"]
WORKDIR /work/src
ARG SOURCE_REVISION
ENV GITHUB_SHA=$SOURCE_REVISION

FROM test-tools AS ordinary-checks
COPY --from=linux --chown=nobody:nogroup /out/core /out/core
COPY --from=linux --chown=nobody:nogroup /out/linux /out/linux
COPY --from=linux --chown=nobody:nogroup /out/native /out/native
ENV SKVOZ_CORE_ARTIFACT=/out/core LD_LIBRARY_PATH=/out/core/lib \
    SKVOZ_TEST_READY_NATIVE_ARCHIVE=/out/linux/skvoz-network-0.7.0-x86_64-unknown-linux-gnu.tar.gz \
    SKVOZ_RUNTIME_EXECUTABLE=/out/native/bin/skvoz-network-runtime
RUN --mount=type=bind,target=/work \
    export SKVOZ_TEST_READY_NATIVE_SHA256=$(sha256sum "$SKVOZ_TEST_READY_NATIVE_ARCHIVE" | cut -d ' ' -f 1) \
    && features=skvoz-network/linux-runtime,skvoz-network/real-nats,skvoz-network-helper/fixture \
    && cargo fmt --all -- --check \
    && cargo test --workspace --features "$features" --release --locked -- --test-threads=1 \
    && cargo clippy --workspace --features "$features",skvoz-network-qualification/experiments --all-targets --release --locked -- -D warnings \
    && cargo test -p skvoz-network-artifact --test native_bundle --release --locked -- --ignored --test-threads=1 \
    && cargo test -p skvoz-network --features linux-runtime --test installed_runtime --release --locked -- --ignored --test-threads=1 --nocapture

FROM scratch AS linux-artifacts
COPY --from=ordinary-checks /out/linux/ /

FROM ordinary-checks AS release
COPY --from=sdks /out/sdks /out/sdks
RUN mkdir /out/release && cp /out/sdks/*.tar.gz /out/linux/*.tar.gz /out/release/ \
    && cd /out/release && sha256sum *.tar.gz > SHA256SUMS

FROM scratch AS artifacts
COPY --from=release /out/release/ /

# The test library builds this final OS-only target without a target selector.
FROM test-os AS test-environment
