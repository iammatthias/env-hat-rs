# Development

## Run the tests

```bash
cargo test --workspace --locked
```

| Test | What it checks |
|---|---|
| Unit tests in `crates/env-hat/src` | Each driver and sampler against a register-model bus (`src/model.rs`). The expected values come from outside the code: datasheet examples, hand-worked formulas, and psychrometric tables. |
| `crates/env-hat/tests/gas_index.rs` | The VOC Index against Sensirion's reference C, including a state save and restore. |

The README's example compiles as a doctest.
`crates/env-hat/tests/gas_index_ref/gen.sh` rebuilds `expected.csv` and
`expected_states.csv` from the reference C sources.

## Run the local CI

CI runs on your machine, not on a hosted service.

```bash
bash ci/run.sh
```

`ci/run.sh` first runs `ci/docs-lint.sh` on the host. The lint checks every
tracked Markdown file for the words the Google style guide bans,
exclamation marks, headings that are not sentence case, and relative links
whose file or anchor does not exist. Code blocks and inline code are
exempt. To check one file, pass its path: `bash ci/docs-lint.sh docs/api.md`.

Then `ci/run.sh` builds an arm64 Debian trixie image, the Pi's platform
and glibc. In that image it runs these checks, in order:

1. `cargo fmt --all --check`, against the root `rustfmt.toml`.
2. `cargo clippy --workspace --all-targets --locked -- -D warnings`.
3. The tests.
4. `cargo audit --deny warnings` on `Cargo.lock`.
5. A release build.

When the host is not Linux, it also runs the tests on the host. To run
the CI before every push, set the hook path once per clone:

```bash
git config core.hooksPath .githooks
```

## Add a dependency

`env-hat` depends on `libc` only.

Every added crate is code from another author running unattended on the
device. A new dependency needs a reason that the standard library and a
page of code can't cover. That rules out argument parsers, logging
frameworks, and async runtimes. For the same reason, the VOC state file is
three numbers separated by spaces, which needs no parser crate. Update
`Cargo.lock` in the same commit as any manifest change.

## Test on the live board

`crates/env-hat/examples/soak.rs` runs `Hat` on the live bus through a
wrapper that counts failed transactions per address and timestamps every
SGP40 measurement. It prints a reading every 10 s. At the end it prints
the bus errors, the share of SGP40 intervals within 1.0 ± 0.1 s, the
acceleration and field magnitudes, and how many snapshots had no
magnetometer field.

Only one program can use the bus at a time. A soak once ran next to
another program that drives the same chips. It froze the magnetometer and
caused about 15 minutes of SGP40 errors.

To run the soak on the Pi, follow these steps:

1. Build the example for the Pi in the CI image, which `ci/run.sh` builds:

   ```bash
   docker run --rm -v "$PWD":/work -v env-hat-cargo:/root/.cargo/registry \
   	-e CARGO_TARGET_DIR=/work/target/ci env-hat-ci \
   	cargo build --release --locked --example soak
   ```

2. Copy it to the Pi:

   ```bash
   ssh <user>@<pi-host> mkdir -p hat-test
   scp target/ci/release/examples/soak <user>@<pi-host>:hat-test/
   ```

   Replace `<user>` and `<pi-host>` with your user name and the Pi's host
   name.

3. Stop whatever service owns the bus, run the soak, and start the service
   again even if the soak fails. This runs it for 330 s, with an optional
   state directory for the VOC baseline:

   ```bash
   ssh <user>@<pi-host> 'trap "sudo systemctl start <service>" EXIT;
   	sudo systemctl stop <service>; ~/hat-test/soak 330 ~/hat-test/state'
   ```

   Replace `<service>` with the name of that service.

4. When you are done, delete `~/hat-test` on the Pi.
