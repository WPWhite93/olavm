use core::{
    types::{Field, GoldilocksField},
    vm::transaction::TxCtxInfo,
};
use std::{
    fs::File,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use ethereum_types::H256;
use executor::BatchCacheManager;
use ola_lang_abi::{Abi, Param, Value};
use plonky2::hash::utils::bytes_to_u64s;

use crate::{
    subcommands::parser::FromValue,
    utils::{address_from_hex_be, h256_to_u64_array, ExpandedPathbufParser, OLA_RAW_TX_TYPE},
};

use super::parser::ToValue;
use zk_vm::OlaVM;

#[derive(Debug, Parser)]
pub struct Call {
    #[clap(long, help = "Path of rocksdb database")]
    db: Option<PathBuf>,
    #[clap(long, help = "Caller Address")]
    caller: Option<String>,
    #[clap(long, help = "Provide block number manually")]
    block: Option<u64>,
    #[clap(long, help = "Provide second timestamp manually")]
    timestamp: Option<u64>,
    #[clap(
        value_parser = ExpandedPathbufParser,
        help = "Path to the JSON keystore"
    )]
    abi: PathBuf,
    #[clap(help = "One or more contract calls. See documentation for more details")]
    calls: Vec<String>,
}

impl Call {
    pub fn run(self) -> anyhow::Result<()> {
        let caller_address: [u64; 4] = if let Some(addr) = self.caller {
            let bytes = address_from_hex_be(addr.as_str()).unwrap();
            let caller_vec = bytes_to_u64s(&bytes);
            let mut caller = [0u64; 4];
            caller.clone_from_slice(&caller_vec[..4]);
            caller
        } else {
            h256_to_u64_array(&H256::random())
        };

        let block_number = if let Some(n) = self.block { n } else { 0 };
        let block_timestamp = if let Some(n) = self.timestamp {
            n
        } else {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
        };
        let db_home = match self.db {
            Some(path) => path,
            None => PathBuf::from("./db"),
        };
        let tree_db_path_buf = db_home.join("tree");
        let state_db_path_buf = db_home.join("state");

        let mut arg_iter = self.calls.into_iter();
        let contract_address_hex = arg_iter.next().expect("contract address needed");
        let contract_address_bytes = address_from_hex_be(contract_address_hex.as_str()).unwrap();
        let to_vec = bytes_to_u64s(&contract_address_bytes);
        let mut to = [0u64; 4];
        to.clone_from_slice(&to_vec[..4]);

        let abi_file = File::open(self.abi).expect("failed to open ABI file");
        let function_sig_name = arg_iter.next().expect("function signature needed");
        let abi: Abi = serde_json::from_reader(abi_file)?;
        let func = abi
            .functions
            .iter()
            .find(|func| func.name == function_sig_name)
            .expect("function not found");
        let func_inputs = &func.inputs;
        if arg_iter.len() != func_inputs.len() {
            anyhow::bail!(
                "invalid args length: {} args expected, you input {}",
                func_inputs.len(),
                arg_iter.len()
            )
        }
        let param_to_input: Vec<(&Param, String)> =
            func_inputs.into_iter().zip(arg_iter.into_iter()).collect();
        let params: Vec<Value> = param_to_input
            .iter()
            .map(|(p, i)| ToValue::parse_input((**p).clone(), i.clone()))
            .collect();
        let calldata = abi
            .encode_input_with_signature(func.signature().as_str(), params.as_slice())
            .unwrap();

        let tx_init_info = TxCtxInfo {
            block_number: GoldilocksField::from_canonical_u64(block_number),
            block_timestamp: GoldilocksField::from_canonical_u64(block_timestamp),
            sequencer_address: [GoldilocksField::ZERO; 4],
            version: GoldilocksField::from_canonical_u32(OLA_RAW_TX_TYPE),
            chain_id: GoldilocksField::from_canonical_u64(1027),
            caller_address: caller_address.map(|n| GoldilocksField::from_canonical_u64(n)),
            nonce: GoldilocksField::ZERO,
            signature_r: [0; 4].map(|n| GoldilocksField::from_canonical_u64(n)),
            signature_s: [0; 4].map(|n| GoldilocksField::from_canonical_u64(n)),
            tx_hash: [0; 4].map(|n| GoldilocksField::from_canonical_u64(n)),
        };

        let mut vm = OlaVM::new_call(
            tree_db_path_buf.as_path(),
            state_db_path_buf.as_path(),
            tx_init_info,
        );
        let exec_res = vm.execute_tx(
            to.map(|n| GoldilocksField::from_canonical_u64(n)),
            to.map(|n| GoldilocksField::from_canonical_u64(n)),
            calldata
                .iter()
                .map(|n| GoldilocksField::from_canonical_u64(*n))
                .collect(),
            &mut BatchCacheManager::default(),
            false,
        );

        match exec_res {
            Ok(_) => {
                let ret_data = vm.ola_state.return_data;
                let u64_ret: Vec<u64> = ret_data.iter().map(|fe| fe.0).collect();
                let decoded = abi
                    .decode_output_from_slice(func.signature().as_str(), &u64_ret)
                    .unwrap();
                println!("Return data:");
                for dp in decoded.1.reader().by_index {
                    let value = FromValue::parse_input(dp.value.clone());
                    println!("{}", value);
                }
            }
            Err(e) => {
                eprintln!("Invoke TX Error: {}", e)
            }
        }
        Ok(())
    }
}
