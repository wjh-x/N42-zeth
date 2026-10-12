<!-- BEGIN WJH RESEARCH OVERVIEW -->
# N42-zeth — zkVM Block Proving for a Custom EVM-Compatible Chain

**Upstream:** [Zeth](https://github.com/boundless-xyz/zeth) · **Repository:** [`wjh-x/N42-zeth`](https://github.com/wjh-x/N42-zeth)  
**Related:** [risc0-v2](https://github.com/wjh-x/risc0-v2) (prover optimizations) · [zeth-v2](https://github.com/wjh-x/zeth-v2) (Ethereum evaluation workflow)

This repository integrates the **N42 Ethereum-compatible execution view** into a Zeth/RISC Zero block-proving workflow. It covers chain-specific EVM configuration, guest selection, witness proxying, proving, receipt selection and host validation. N42 node-side historical state and RPC compatibility work is a **separate dependency**, not code supplied solely by this repository.

**Proof boundary:** the evaluated receipt attests to execution under the selected **compatible EVM guest and supplied execution checkpoint**. It does **not** by itself prove native N42 consensus, finality, or an independently authenticated mapping from native N42 history to a reconstructed Ethereum-compatible header/state root.

## Implemented N42 integration

| Code area | Responsibility |
| --- | --- |
| [`crates/chainspec/src/lib.rs`](https://github.com/wjh-x/N42-zeth/blob/main/crates/chainspec/src/lib.rs) | Chain-specific specifications for N42 (`chainId=94`) and N42 ethdev (`chainId=1337`) with compatible EVM fork selection. |
| `guests/stateless-client/src/bin/n42.rs` | Guest entry point for the N42-compatible execution profile. |
| `guests/stateless-client/src/bin/n42_ethdev.rs` | Guest entry point for the controlled N42 ethdev environment. |
| `crates/host/src/lib.rs` | Select the guest ELF and execution path from the RPC chain identity; execute validation and proving. |
| `crates/rpc-proxy/src/main.rs` | Choose N42/N42-ethdev EVM configuration and bridge witness-related RPC behavior. |
| `crates/host/src/bin/cli.rs` | Host CLI, `--segment-po2`, and receipt-kind selection: `composite` (default), `succinct`, `groth16`. |
| `crates/host/tests/e2e.rs` | End-to-end host/integration test entry points. |

`CompositeReceipt` is the measured generation endpoint in the experiments below. `succinct` and `groth16` are supported CLI choices in the evaluated implementation, **not** proof types covered by the performance numbers shown here. On-chain seal export/ABI integration is not claimed to be complete.

## Historical witness and block-identity requirements

The stateless executor needs **the parent's authenticated state**, not simply an RPC snapshot of the current chain head. The evaluated N42 environment additionally required work in the **N42-gov5 node** to provide:

1. Historical Ethereum Merkle Patricia Trie (MPT) checkpoints and standard EIP-1186 account/storage proofs (development-node experiment flag: `N42_MPT_HISTORY_PROOFS=1`).
2. Consistent parent and target state roots, transaction and receipt commitments, and an Ethereum-compatible header/hash view for the selected guest/fork rules.
3. Lookup of the correct historical state when a compatible block hash is passed to `eth_getProof` (instead of silently using the latest state).
4. Correct account-scoped storage paths and historical storage-slot values, including nonzero mapping slots.

These fixes are **node/provider-side prerequisites**. They must not be described as modifications to the RISC Zero proof protocol or attributed to the Zeth repository alone. For an untrusted native-chain setting, applications still need to authenticate the native-to-compatible checkpoint binding independently.

## Running the N42 ethdev proving workflow

Start a compatible N42 ethdev node that exposes historical block data, `eth_getProof`, and a correct execution-witness interface. The following localhost addresses are **examples**, not public RPC endpoints. Do not commit keystores, RPC credentials or node databases.

Terminal 1 — start the N42-aware Zeth proxy:

```bash
cargo run --release --bin zeth-rpc-proxy -- \
  --rpc-url http://127.0.0.1:8545 \
  --bind-address 127.0.0.1:18545
```

Terminal 2 — use a CUDA-enabled RISC Zero prover to generate and verify an ethdev block receipt:

```bash
ETH_RPC_URL=http://127.0.0.1:18545 \
RISC0_PROVER=ipc \
RISC0_SERVER_PATH=/absolute/path/to/risc0/target/release/r0vm \
cargo run --release --bin cli -- \
  prove --block 5 --segment-po2 20 --receipt-kind composite
```

`block 5` is an **illustrative historical ethdev input**, not the 24-transaction benchmark below. Select an actual block and supported fork configuration in your own controlled node. For older/pre-merge profiles, the evaluated workflow may require building Zeth with `--features unsafe-pre-merge` as appropriate to the selected source version.

Prover performance switches are implemented in [risc0-v2](https://github.com/wjh-x/risc0-v2). To exercise the full stack, use its optimized `r0vm` **built with the desired compile-time code-generation flags**, and pass its runtime flags to the prover process. Building or running this repository against an unmodified RISC Zero prover does not implicitly enable those optimizations.

## Evaluated workloads and results

All timing figures here refer to **Composite receipt generation**, excluding the subsequent verification phase. Every accepted run completed receipt verification and the expected journal identity checks. Measurements were collected on **one RTX 5080** with fixed per-workload inputs.

| N42 ethdev workload | What was validated | Measurement scope |
| --- | --- | --- |
| **Small transfer block** | EOA transfers and historical account proof paths, including a five-transaction block | Earlier single-workload optimization observations; not evidence for contract storage coverage. |
| **StorageProbe single-contract transition** | One contract, nonempty parent slot 0, `1 → 2` storage update, nonempty code and storage proof in witness | A verified single-segment proof; older full-stack-only five-run mean **2.2354 s** is correctness/scale evidence, not a speedup without its own baseline. |
| **Controlled mixed 24-transaction block** | Two contracts, three nonempty storage slots, four EOA transfers, 10 RV32IM segments, account/storage MPT paths | Five interleaved baseline/full-stack pairs comparing the *same* block, guest and witness. |

### Mixed multi-segment workload (five paired runs)

The controlled synthetic ethdev block contains **24 transactions**: 12 increments on `StorageProbeA`, four increments on each of two `StorageProbeB` mapping keys, and four EOA transfers. The three parent storage slots are authenticated as nonempty; this is **not** an empty-storage-only test.

| Prover configuration | Composite generation mean (n=5) |
| --- | ---: |
| Baseline, all prover performance optimizations off | **11.6650 s** |
| Full optimized prover | **7.2402 s** |
| Difference | **4.4248 s** (**37.93% reduction**, **1.61×** speedup) |

There are **10 RV32IM segments**, so one-segment lookahead actually applies. The measured pipeline model estimates about **1.93 s** of overlapped work in the optimized runs; the entire 4.4248 s difference must **not** be attributed to pipelining because FRI, code-generation, pool, BigInt and host-lifetime optimizations also changed.

The test used a **controlled synthetic workload** with a fixed guest, exact cached input/hash, the same N42 compatibility logic and **8 GiB CUDA pool cap where enabled**. It is not representative of every N42 block or a production validator benchmark. The historical N42 node's storage-history bug was corrected and checked **before** the measured control and treatment runs; that correctness repair is **not** an RISC Zero performance optimization.

## Correctness, trust and reproducibility

- The supported N42 ethdev profile is an **Ethereum-compatible execution view**; its header and state root must agree with the guest's exact fork rules.
- The host/guest workflow checks the configured execution commitments, and the application must verify the **expected** guest image ID and journal/output encoding. Do not equate a host-side check or a valid receipt with native N42 consensus finality.
- Node-side historical MPT proofs, account-scoped storage paths and compatible-hash lookup must be correct before benchmarking.
- The mixed workload, storage transition and older transfer blocks are **different benchmarks**. Their times must not be subtracted from one another to claim optimization gains.
- Benchmarks are scoped to the evaluated GPU and receipt type; controlled runs do not establish performance on other networks or GPUs.

## Attribution

Based on upstream **Zeth**, **RISC Zero** and **Reth**; original licenses and notices remain in effect. This repository's added work is the N42-specific **Zeth integration**, while the node-side MPT/history provider and low-level RISC Zero optimizations are separate components. This is a research prototype, not an official N42/Zeth release or a production security audit.
<!-- END WJH RESEARCH OVERVIEW -->

---

## Original upstream README

# Zeth: A RISC Zero zkVM Block Prover for Ethereum

Zeth is an open-source, execution-layer block prover for Ethereum, built on the RISC Zero zkVM and leveraging the power of [Reth](https://reth.rs/).

By using Reth's stateless execution capabilities within the zkVM, Zeth makes it possible to generate a cryptographic proof of a block's validity. This allows you to verify the entire block execution process—from transaction validation to state root updates—without trusting a third-party node provider.

## How It Works

Zeth streamlines the process of proving block execution by taking advantage of reth's stateless execution feature. The core logic involves:

1. **Fetching Data**: Zeth requires an archival Ethereum RPC provider to fetch the block header and an "execution witness." The witness contains all the necessary pre-state data (account info, storage slots, bytecodes) required to execute the block from scratch.
2. **Stateless Execution**: The execution witness and the block data are provided as inputs to the RISC Zero zkVM.
3. **Proving**: Inside the zkVM, the guest program uses reth's stateless validation function to execute all transactions in the block, apply rewards, and compute the final state root.
4. **Journal Output**: The guest program commits the calculated state root to the public journal. The host can then verify this against the known, correct state root from the block header.

## Prerequisites

You'll need the following installed to run Zeth:

1. [Rust](https://www.rust-lang.org/tools/install) (see rust-toolchain.toml for the exact version).
2. The [RISC Zero toolchain](https://dev.risczero.com/api/zkvm/install).

## Building the Project

To build the host CLI and guest programs, run:
```bash
cargo build --release
```

### Deterministic Guest Builds with Docker

For ZK proofs, it is critical that the guest binary is built deterministically. This ensures that every developer builds the exact same guest program, resulting in the same Image ID, which is essential for verification.

This project is configured to use Docker to achieve reproducible builds. If you have Docker installed, you can enable this feature by setting an environment variable:
```bash
RISC0_USE_DOCKER=1 cargo build --release
```
This will build the guest programs inside a controlled Docker environment, guaranteeing a deterministic output. The project's release workflow uses this method to build the official guest binaries.

## Usage

Zeth requires an archival Ethereum RPC provider to fetch block and state data. You can provide this using the `ETH_RPC_URL` environment variable or the `--eth-rpc-url` command-line argument.

### RPC Provider Requirements

Zeth's core functionality depends on the non-standard `debug_executionWitness` RPC method to generate the execution witness. Not all RPC providers support this method.
If your provider does not support `debug_executionWitness`, you can use the included `zeth-rpc-proxy`. This proxy server will intercept requests, generate the witness locally using a standard archival node, and forward all other requests to your provider. You can run it with:

```bash
ETH_RPC_URL="<YOUR_ARCHIVAL_RPC_URL>" cargo run --release --bin zeth-rpc-proxy
```

You can then point the Zeth CLI to the proxy, which runs on `127.0.0.1:8545` by default.

### CLI Commands

The CLI provides two main commands for interacting with blocks.

```
$ cargo run --bin cli -- --help

Simple CLI to create Ethereum block execution proofs

Usage: cli [OPTIONS] --eth-rpc-url <ETH_RPC_URL> <COMMAND>

Commands:
  prove     Validate the block and generate a RISC Zero proof
  validate  Validate the block on the host machine, without proving
  help      Print this message or the help of the given subcommand(s)

Options:
      --eth-rpc-url <ETH_RPC_URL>  URL of the Ethereum RPC endpoint to connect to [env: ETH_RPC_URL=]
      --block <BLOCK>              Block number, tag, or hash (e.g., "latest", "0x1565483") to execute [default: latest]
      --cache-dir <CACHE_DIR>      Cache folder for input files [default: ./cache]
  -h, --help                       Print help
  -V, --version                    Print version
```

### `validate`

This command runs the block execution logic on your local machine (the "host") without generating a proof. It's useful for quickly verifying that a block can be correctly processed and for populating the local cache.

#### Validate the latest block
```bash
ETH_RPC_URL="<YOUR_RPC_URL>" cargo run --release --bin cli -- validate
```

#### Validate a specific block by number
```bash
ETH_RPC_URL="<YOUR_RPC_URL>" cargo run --release --bin cli -- validate
```

Upon first run, this will fetch the necessary data from the RPC and save it to the cache/ directory. Subsequent runs for the same block will be much faster as they will use the cached data.

### `prove`

This command first validates the block on the host and then proceeds to generate a full cryptographic proof of execution inside the RISC Zero zkVM.

#### Prove the latest block  
```bash
ETH_RPC_URL="<YOUR_RPC_URL>" cargo run --release --bin cli -- prove
```

**Developer Mode**: For faster iteration during development, you can generate a "mock" proof by enabling RISC Zero's dev mode. This runs the guest code in the zkVM but skips the expensive proof generation step.

#### Generate a mock proof for a specific block
```bash
RISC0_DEV_MODE=1 ETH_RPC_URL="<YOUR_RPC_URL>" cargo run --release --bin cli -- prove 19000000
```

#### Proving Pre-Merge (Proof-of-Work) Blocks

By default, Zeth only supports proving post-merge (Proof-of-Stake) blocks. This is because the underlying `reth-stateless` library does not fully validate the total difficulty, making proofs of pre-merge blocks underconstrained.

For development and research purposes, you can enable the proving of historical blocks by using the `unsafe-pre-merge` feature flag:
```bash
ETH_RPC_URL="<YOUR_RPC_URL>" cargo run --release --bin cli --features "unsafe-pre-merge" -- prove --block 1
```

**Warning**: Proofs generated with this flag are not fully sound and should not be used in production.

## Additional Resources

* [RISC Zero Developer Portal](https://dev.risczero.com/)
* [reth \- The Rust Ethereum Book](https://reth.rs/)
* Say hi on the [RISC Zero Discord](https://discord.gg/risczero)
