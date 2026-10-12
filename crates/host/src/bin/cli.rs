// Copyright 2026 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use alloy::{eips::BlockId, primitives::B256, providers::ProviderBuilder};
use anyhow::{Context, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use humansize::{DECIMAL, format_size};
use risc0_zkvm::ProverOpts;
use std::{fs, path::PathBuf};
use tracing::{Instrument, info, info_span};
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt::format::FmtSpan};
use zeth_host::{BlockProcessor, to_zkvm_input_bytes};

/// Simple CLI to create Ethereum block execution proofs.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Ethereum RPC endpoint URL.
    #[arg(long, env)]
    eth_rpc_url: String,

    /// Block to execute: number (e.g., 21000000), tag ("latest"), or hash ("0xabcd...").
    #[arg(long, global = true, default_value = "latest")]
    block: BlockId,

    /// Directory for caching block inputs.
    #[arg(long, global = true, default_value = "./cache")]
    cache_dir: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum Commands {
    /// Validate the block and generate a RISC Zero proof.
    Prove(ProveCommand),

    /// Validate the block on the host machine, without proving.
    Validate,
}

#[derive(Parser, Debug, PartialEq, Eq)]
struct ProveCommand {
    /// Segment size as a power of 2 (e.g., 20 = 1M cycles per segment).
    #[arg(long, env)]
    segment_po2: Option<u32>,

    /// Receipt type: composite is fastest, succinct is compressed, groth16 is for on-chain verification.
    #[arg(long, value_enum, default_value_t = ProofKind::Composite)]
    receipt_kind: ProofKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ProofKind {
    Composite,
    Succinct,
    Groth16,
}

impl ProofKind {
    fn prover_opts(self) -> ProverOpts {
        match self {
            Self::Composite => ProverOpts::composite(),
            Self::Succinct => ProverOpts::succinct(),
            Self::Groth16 => ProverOpts::groth16(),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    //总开始时刻
    let t_total = std::time::Instant::now();
    setup_tracing();

    // This is a hack to ensure that `blst` gets linked into this binary.
    let _ = unsafe { blst::blst_p1_sizeof() };

    let cli = Cli::parse();

    fs::create_dir_all(&cli.cache_dir).context("failed to create cache directory")?;

    let provider =
        ProviderBuilder::new().connect(&cli.eth_rpc_url).await.context("RPC connection failed")?;
    let processor = BlockProcessor::new(provider).await?;

    info!(chain = %processor.chain(), "Initialized block processor");

    // 获取区块开始时刻
    let t_fetch = std::time::Instant::now();
    let input = processor
        .get_input_with_cache(cli.block, &cli.cache_dir)
        .instrument(info_span!("retrieve_input"))
        .await?;
    // 获取区块所用时长
    let fetch_elapsed = t_fetch.elapsed();
    let block_hash = input.block.hash_slow();
    let input_bytes = to_zkvm_input_bytes(&input).len();
    let state_bytes: usize = input.witness.state.iter().map(|v| v.len()).sum();
    let code_bytes: usize = input.witness.codes.iter().map(|v| v.len()).sum();
    let key_bytes: usize = input.witness.keys.iter().map(|v| v.len()).sum();
    let header_bytes: usize = input.witness.headers.iter().map(|v| v.len()).sum();

    info!(
        block_number = input.block.number,
        %block_hash,
        size = %format_size(input_bytes, DECIMAL),
        "Retrieved input for block",
    );
    // 论文实验指标：分解stateless witness大小，total_input_bytes是实际写入zkVM的序列化输入。
    println!(
        "[METRIC] witness_size block:{} txs:{} total_input_bytes:{} state_nodes:{} state_bytes:{} code_entries:{} code_bytes:{} key_entries:{} key_bytes:{} header_entries:{} header_bytes:{}",
        input.block.number,
        input.block.body.transactions().count(),
        input_bytes,
        input.witness.state.len(),
        state_bytes,
        input.witness.codes.len(),
        code_bytes,
        input.witness.keys.len(),
        key_bytes,
        input.witness.headers.len(),
        header_bytes,
    );

    println!("[TIMING] 1. 获取区块: {:.3}s", fetch_elapsed.as_secs_f64());
    // 验证区块开始时刻
    let t_validate = std::time::Instant::now();
    info_span!("validate")
        .in_scope(|| processor.validate(input.clone()))
        .context("host validation failed")?;
    let validate_elapsed = t_validate.elapsed();
    info!("Host validation successful");
    println!("[TIMING] 2. 验证区块: {:.3}s", validate_elapsed.as_secs_f64());

    if let Commands::Prove(ProveCommand { segment_po2, receipt_kind }) = cli.command {
        // 证明生成开始时刻
        let t_prove = std::time::Instant::now();
        let (receipt, image_id, stats) = processor
            .prove_with_opts(input, segment_po2, receipt_kind.prover_opts())
            .instrument(info_span!("prove"))
            .await
            .context("proving failed")?;
        let prove_elapsed = t_prove.elapsed();
        println!("[TIMING] 证明类型: {receipt_kind:?}");
        println!("[TIMING] 3. 生成证明: {:.3}s", prove_elapsed.as_secs_f64());
        // 论文实验指标：RISC0返回的guest cycles和segment数，用于规模扩展曲线。
        println!(
            "[METRIC] guest_cycles segments:{} total:{} user:{} paging:{} reserved:{}",
            stats.segments,
            stats.total_cycles,
            stats.user_cycles,
            stats.paging_cycles,
            stats.reserved_cycles
        );

        // 验证证明开始时刻
        let t_verify = std::time::Instant::now();
        receipt.verify(image_id).context("proof verification failed")?;
        // 验证证明所用时长
        let verify_elapsed = t_verify.elapsed();
        println!("[TIMING] 4. 验证证明: {:.3}s", verify_elapsed.as_secs_f64());

        let proven_hash =
            B256::try_from(receipt.journal.as_ref()).context("failed to decode journal")?;
        ensure!(proven_hash == block_hash, "journal output mismatch");
        info!(%block_hash, "Proving successful");

        // 总时长
        println!("----------------------------------------");
        println!("[TIMING] 汇总:");
        println!("  1. 获取区块:  {:.3}s", fetch_elapsed.as_secs_f64());
        println!("  2. 验证区块:  {:.3}s", validate_elapsed.as_secs_f64());
        println!("  3. 生成证明:  {:.3}s", prove_elapsed.as_secs_f64());
        println!("  4. 验证证明:  {:.3}s", verify_elapsed.as_secs_f64());
        println!(
            "  各阶段之和:   {:.3}s",
            (fetch_elapsed + validate_elapsed + prove_elapsed + verify_elapsed).as_secs_f64()
        );
        println!("  程序总耗时:   {:.3}s", t_total.elapsed().as_secs_f64()); // ← 真实总时长
        println!("----------------------------------------");
    }

    Ok(())
}

fn setup_tracing() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_span_events(FmtSpan::CLOSE)
        .with_env_filter(
            EnvFilter::builder().with_default_directive(LevelFilter::INFO.into()).from_env_lossy(),
        )
        .init();
}
