# Runs the pyln-testing suite in a container, the way CI does:
#   docker build -f tests/Dockerfile -t cln-mint-tests . && docker run --rm cln-mint-tests
# Pass `-e LNURLCASHKERNEL_LIB=...` with a mounted libbitcoinkernel to cover tapscript leaves.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests/vectors ./tests/vectors
RUN cargo build --locked && cargo test --locked

FROM elementsproject/lightningd:v26.06.8
ARG BITCOIND_VERSION=31.0
RUN apt-get update && apt-get install -y --no-install-recommends python3 python3-venv curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://bitcoincore.org/bin/bitcoin-core-${BITCOIND_VERSION}/bitcoin-${BITCOIND_VERSION}-$(uname -m)-linux-gnu.tar.gz \
    | tar -xz -C /tmp && mv /tmp/bitcoin-${BITCOIND_VERSION}/bin/* /usr/local/bin/ && rm -rf /tmp/bitcoin-*
RUN python3 -m venv /venv && /venv/bin/pip install --no-cache-dir \
    "pytest>=9,<10" "pytest-xdist>=3.7,<4" "pytest-timeout>=2.4,<3" requests \
    "pyln-testing==26.6.8" "pyln-client==26.6.8" "pyln-proto==26.6.8"
WORKDIR /plugin
COPY --from=build /src/target/debug/cln-mint target/debug/cln-mint
COPY tests ./tests
ENV SLOW_MACHINE=1 TEST_DEBUG=1 VALGRIND=0 PYTEST_TIMEOUT=600
WORKDIR /plugin/tests
ENTRYPOINT ["/venv/bin/pytest", "-n", "4", "-v"]
