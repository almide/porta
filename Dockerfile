# Porta Enterprise evaluation image: `porta job-serve` with the sample policy.
#
#   docker build -t porta-eval .
#   docker run --rm -p 8080:8080 -e PORTA_JOB_TOKEN=<16+ chars> porta-eval
#
# Stages: `build` compiles porta from this checkout with the pinned Almide;
# `test` runs the job integration suite on that binary as an unprivileged
# user; `runtime` is the image that ships. Build `--target test` to run the
# suite; the default target does not.

FROM rust:1-trixie AS build
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends python3 curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*
RUN useradd --create-home dev
USER dev
WORKDIR /home/dev/src
COPY --chown=dev . .
RUN rm -rf target .tools \
 && bash scripts/install-almide.sh \
 && .tools/almide/almide build src/main.almd -o target/porta \
 && strip target/porta

FROM build AS test
RUN python3 scripts/job_integration.py target/porta

FROM debian:trixie-slim AS runtime
ARG PORTA_UID=10001
# libssl: porta's HTTP client links OpenSSL on Linux (see THIRD_PARTY_LICENSES.md).
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends libssl3t64 ca-certificates \
 && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid ${PORTA_UID} --create-home --home-dir /var/lib/porta porta
COPY --from=build /home/dev/src/target/porta /usr/local/bin/porta
COPY deploy/container/policy.toml deploy/container/policy-wasm-only.toml /etc/porta/
COPY examples/enterprise/sample-job/sample-job.wasm /opt/porta/modules/sample-job.wasm
COPY examples/enterprise/catalog/ /opt/porta/catalog/
COPY LICENSE THIRD_PARTY_LICENSES.md /usr/share/doc/porta/
USER porta
ENV HOME=/var/lib/porta
# Fails the build when a policy's pinned digest does not match the module.
RUN porta job-check --policy /etc/porta/policy.toml > /dev/null \
 && porta job-check --policy /etc/porta/policy-wasm-only.toml > /dev/null
EXPOSE 8080
STOPSIGNAL SIGTERM
# policy.toml needs the OS sandbox and refuses to start where the kernel
# cannot apply it; policy-wasm-only.toml runs on the WASM boundary alone.
ENTRYPOINT ["porta", "job-serve"]
CMD ["--policy", "/etc/porta/policy.toml", "--listen", "0.0.0.0:8080"]
