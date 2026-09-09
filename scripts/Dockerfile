FROM rust:1.85-bookworm

RUN rustup toolchain install 1.85.0 --profile minimal \
    && rustup toolchain install stable --profile minimal \
        --component rustfmt --component clippy --component llvm-tools-preview \
    && rustup default stable \
    && cargo install cargo-llvm-cov --version 0.8.7 --locked

ENV CARGO_TARGET_DIR=/target
WORKDIR /work

CMD ["bash", "-c", "cargo +1.85.0 check --all-targets --locked && bash scripts/verify.sh"]
