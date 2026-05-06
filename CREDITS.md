# Credits & Acknowledgements

Sleepyminer stands on the work of others. The portions below describe what
was reused and how, with the goal of giving proper credit and making
license inheritance clear.

## RandomX algorithm

The RandomX proof-of-work algorithm and its reference C++ implementation
are the work of **tevador** and contributors. The reference library lives
at <https://github.com/tevador/RandomX> and is licensed under the
3-clause BSD license. Sleepyminer vendors a copy of that source tree
under `vendor/randomx/` with the upstream license headers preserved
verbatim on every file.

If you use sleepyminer to mine RandomX coins, the heavy lifting at
runtime — the cache build, the dataset, the VM, the AES helpers — is
tevador's code, possibly with the local modifications described in the
next section.

## Hot-path optimizations on top of RandomX

Three files in `vendor/randomx/src/` carry local modifications relative
to the upstream tevador library:

- `jit_compiler_a64_static.S` (ARM64 assembly outer loop)
- `jit_compiler_a64.cpp` (ARM64 JIT compiler)
- `aes_hash.cpp` (AES scratchpad helpers)

These files were derived from the corresponding files in the
**xmrig** project (<https://github.com/xmrig/xmrig>), which is
licensed under GPL-3.0 with copyrights held by:

- 2018-2019, tevador <tevador@gmail.com>
- 2019-2026, SChernykh <https://github.com/SChernykh>
- 2019-2026, XMRig <https://github.com/xmrig>, <support@xmrig.com>

xmrig has spent years hand-tuning these paths for ARM64. The local
modifications on top of xmrig's versions are limited and were generated
through automated search against the standalone RandomX test suite and
benchmark; they preserve the same hash output as upstream RandomX
(verified by the 105-test correctness suite shipped with the library).

Specifically:

- `jit_compiler_a64_static.S`: a `dup`-broadcast in place of paired
  `ins` instructions for the v29/v31 mantissa masks; `ldp`-batched
  literal loading in the prologue; `scvtf` interleaved with loads in
  the main-loop F/E-register setup to start FP pipelines earlier on
  Apple Silicon's deep out-of-order window.
- `jit_compiler_a64.cpp`: a one-line change to set the Apple-specific
  branch-likely hint bit in the JIT-emitted `b.eq` for `CBRANCH`.
- `aes_hash.cpp`: kept at the xmrig-ported version (no further local
  changes shipped after benchmarking confirmed the alternatives were
  within run-to-run noise).

Because the modified files are derived from GPL-3.0 source, the entire
sleepyminer project is distributed under GPL-3.0 (see `LICENSE`).

## Inspirations

The shape of the miner — adaptive thread scaling, donation
time-slicing, the wizard-driven first-run experience — was informed
by reading the source of:

- **xmrig** (<https://github.com/xmrig/xmrig>) — the reference Monero
  CPU miner; in particular its scheduling heuristics and donation
  mechanism. xmrig's design conventions shaped how sleepyminer thinks
  about thread quality-of-service and the dev-fee accounting model.
- **xmrig-mo** (<https://github.com/MoneroOcean/xmrig>) — MoneroOcean's
  fork of xmrig; the `client.reconnect` stratum support in this
  miner was added after observing how MoneroOcean's pool relies on
  it for load-balancer rotation.

These are inspirations only; no source code was lifted from these
projects beyond the RandomX hot paths described above.

## Rust dependencies

The Rust side of sleepyminer relies on the Cargo ecosystem; the
dependency list is in `Cargo.toml`. Notable runtime dependencies:

- `tokio` — async runtime
- `tokio-rustls`, `rustls`, `webpki-roots` — TLS for stratum-over-TLS
  pools (e.g. NiceHash)
- `clap` — CLI parsing
- `serde` / `serde_json` — pool protocol marshalling
- `sha2` / `sha3` / `hex` — wallet-derived donation IDs
- `num_cpus`, `libc`, `log` — OS plumbing
- `reqwest` — HTTPS for occasional pool-status calls

The `cmake` build dependency drives the vendored `randomx/` C++ build.

## Reporting

Bugs and feedback go through GitHub Issues on the project's repository.
There is no email contact channel; please open an issue.
