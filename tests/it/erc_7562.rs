//! 7562 tests

use crate::utils::inspect;
use alloy_primitives::{address, U256};
use revm::{
    db::{CacheDB, EmptyDB},
    primitives::{
        AccountInfo, BlobExcessGasAndPrice, BlockEnv, CfgEnv, CfgEnvWithHandlerCfg,
        EnvWithHandlerCfg, HandlerCfg, SpecId, TransactTo, TxEnv,
    },
};
use revm_inspectors::tracing::Erc7562ValidationTracer;

#[test]
fn test_tracer_fuck() {
    let caller = address!("283b5b7d75e3e6b84b8e2161e8a468d733bbbe8d");

    let mut db = CacheDB::new(EmptyDB::default());

    let cfg = CfgEnvWithHandlerCfg::new(CfgEnv::default(), HandlerCfg::new(SpecId::CANCUN));

    db.insert_account_info(
        caller,
        AccountInfo { balance: U256::from(u64::MAX), ..Default::default() },
    );

    let to = address!("15dd773dad3f630773a0e771e9b221f4c8b9b939");

    let env = EnvWithHandlerCfg::new_with_cfg_env(
        cfg.clone(),
        BlockEnv {
            basefee: U256::from(100),
            blob_excess_gas_and_price: Some(BlobExcessGasAndPrice::new(100, false)),
            ..Default::default()
        },
        TxEnv {
            caller,
            gas_limit: 1000000,
            transact_to: TransactTo::Call(to),
            gas_price: U256::from(150),
            blob_hashes: vec!["0x01af2fd94f17364bc8ef371c4c90c3a33855ff972d10b9c03d0445b3fca063ea"
                .parse()
                .unwrap()],
            max_fee_per_blob_gas: Some(U256::from(1000000000)),
            ..Default::default()
        },
    );

    let mut insp = Erc7562ValidationTracer::new();
    let (res, _) = inspect(&mut db, env, &mut insp).unwrap();
    println!("HELLO: FUCKER {:?}", res.result);
}
