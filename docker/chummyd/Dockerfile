# chummyd in a container: a distroless image (no shell, no package
# manager) that runs as the unprivileged user 65532.
#
# Build from the repository's root, with the tools' Python in
# wasi-python/python-wasi.zip (from a Chummy release, or built with
# scripts/build-wasi-python.sh; see docs/user/chummyd.md):
#
#   docker build -f docker/chummyd/Dockerfile -t chummyd .
#
# The build downloads only the base images, Rust and its crates: the tools'
# Python comes ready-made, so nothing is fetched from GitHub, python.org or
# PyPI.
#
# See docs/user/chummyd.md ("In Docker") and docker/chummyd/compose.yaml.
# x86_64 only for now.

# --- chummyd --------------------------------------------------------------
FROM rust:1-slim-trixie AS build
# pkg-config and libdbus: a shared crate's keychain code needs them to
# build (chummyd never uses the keychain, and doesn't link libdbus: see the
# check below).
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libdbus-1-dev \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# The version pinned in rust-toolchain.toml.
COPY rust-toolchain.toml ./
RUN rustup toolchain install
COPY . .
# The tools' Python, checked against its .sha256 when that came with it.
RUN if [ ! -f wasi-python/python-wasi.zip ]; then \
      echo "The tools' Python is missing: put python-wasi.zip in wasi-python/ (see docs/user/chummyd.md, \"In Docker\")." >&2; \
      exit 1; \
    fi \
 && if [ -f wasi-python/python-wasi.zip.sha256 ]; then \
      (cd wasi-python && sha256sum -c python-wasi.zip.sha256); \
    else \
      echo "Note: no python-wasi.zip.sha256 beside the zip, so its hash wasn't checked." >&2; \
    fi
# Fail rather than leave the tools' Python out.
ENV CHUMMY_REQUIRE_BUNDLED_PYTHON=1 \
    CARGO_INCREMENTAL=0
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build -p chummyd --release --locked \
 && install -D -m 0755 target/release/chummyd /out/chummyd
# Only the C library and libgcc: anything else wouldn't be in the image.
RUN unexpected="$(ldd /out/chummyd | grep -vE 'linux-vdso|libgcc_s\.so|libm\.so|libc\.so|ld-linux' || true)" \
 && if [ -n "$unexpected" ]; then echo "chummyd links more than expected:" >&2; echo "$unexpected" >&2; exit 1; fi
# Folders for the volumes, owned by chummyd's user, so new volumes start
# out with that owner.
RUN mkdir -p /out/bundle /out/jails /out/state \
 && chown 65532:65532 /out/jails /out/state \
 && chmod 0700 /out/jails /out/state

# --- The image --------------------------------------------------------------
# glibc, libgcc, CA certificates and time zones; nothing else. User 65532
# ("nonroot"): chummyd refuses to run as root.
FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build /out/chummyd /usr/local/bin/chummyd
COPY --from=build /out/bundle /bundle
COPY --from=build --chown=65532:65532 /out/jails /jails
COPY --from=build --chown=65532:65532 /out/state /state
USER 65532:65532
LABEL org.opencontainers.image.title="chummyd" \
      org.opencontainers.image.description="Chummy agents, running on their own" \
      org.opencontainers.image.source="https://github.com/friendlylabz/chummy" \
      org.opencontainers.image.licenses="MIT"
# No ports: chummyd listens on nothing.
VOLUME ["/jails", "/state"]
ENTRYPOINT ["/usr/local/bin/chummyd"]
# The bundle mounted read-only at /bundle/bundle.chummyd, the password as a
# secret. Without --password-file (and with a terminal: docker run -it),
# chummyd asks for it instead.
# The agents' folders are /jails/agents/<agent>: a volume's own root is a
# mount point, which can't be a workspace.
CMD ["run", "--bundle", "/bundle/bundle.chummyd", "--jail-dir", "/jails/agents", "--state", "/state", "--password-file", "/run/secrets/chummyd_password"]
