# bevy_dlss_bench: see README.md

dlss_version := "310.9.1"
dlss_so := "vendor/DLSS/lib/Linux_x86_64/rel/libnvidia-ngx-dlss.so." + dlss_version

# Capture every scene and variant, then compose sheets (extra args go to the binary).
shots *ARGS: sdk
    cargo build --release
    # DLSS looks for its runtime library next to the executable.
    cp -f {{dlss_so}} target/release/
    ./target/release/bevy_dlss_bench {{ARGS}}

# Interactive viewer, e.g. `just view --scene ssao --variants dlss-performance`.
view *ARGS: sdk
    cargo build --release
    cp -f {{dlss_so}} target/release/
    ./target/release/bevy_dlss_bench --view {{ARGS}}

# Same without DLSS support compiled in (any GPU / OS): native shots only.
shots-native *ARGS:
    cargo run --release --no-default-features -- {{ARGS}}

# Re-compose sheets from existing captures.
sheets *ARGS:
    cargo run --release -- --sheets-only {{ARGS}}

# List scenes and their variants.
list:
    cargo run --release -- --list

# Fetch the pinned DLSS SDK into vendor/DLSS.
sdk:
    ./scripts/fetch-dlss-sdk.sh

check:
    cargo clippy --all-targets
    cargo test --no-default-features

clean-shots:
    rm -rf out
