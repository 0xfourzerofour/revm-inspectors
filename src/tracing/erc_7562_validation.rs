//! ERC 7562 validation tracer.
//!
//! See also <https://geth.ethereum.org/docs/developers/evm-tracing/built-in-tracers>

use std::fmt::Debug;

use alloy_primitives::{
    map::foldhash::{HashMap, HashSet},
    Address, Bytes, IntoLogData, U256,
};
use revm::{
    interpreter::{Interpreter, OpCode},
    Database, EvmContext, Inspector,
};

use super::types::{CallLog, DecodedCallLog};

#[derive(Clone, Debug)]
pub struct Erc7562ValidationTracer {
    config: Erc7562ValidationTracerConfig,
    gas_limit: u64,
    depth: usize,
    interrupt: bool,
    reason: String,
    // ignoredOpcodes       map[vm.OpCode]struct{}
    callstack_with_opcodes: Vec<CallFrameWithOpCodes>,
    // lastOpWithStack      *opcodeWithPartialStack
    // Keccak               map[string]struct{} `json:"keccak"`
}

#[derive(Clone, Debug)]
pub struct Erc7562ValidationTracerConfig {
    stack_top_items_size: usize,
    ignored_opcodes: HashSet<OpCode>,
    with_log: bool,
}

#[derive(Clone, Debug)]
pub struct CallFrameWithOpCodes {
    ty: OpCode,
    from: Address,
    gas: u64,
    gas_used: u64,
    to: Address,
    input: Bytes,
    output: Bytes,
    error: String,
    revert_reason: String,
    logs: Vec<CallLog>,
    value: U256,
    reverted_snapshot: bool,
    accessed_slots: AccessedSlots,
    ext_code_access_info: Vec<Address>,
    used_opcodes: HashMap<OpCode, u64>,
    contract_size: HashMap<Address, ContractSizeWithOpCode>,
    out_of_gas: bool,
    calls: Vec<CallFrameWithOpCodes>,
}

#[derive(Clone, Debug)]
pub struct AccessedSlots {
    reads: HashMap<String, Vec<String>>,
    writes: HashMap<String, u64>,
    transient_reads: HashMap<String, u64>,
    transient_writes: HashMap<String, u64>,
}

#[derive(Clone, Debug)]
pub struct ContractSizeWithOpCode {
    contract_size: u64,
    opcode: OpCode,
}

#[derive(Clone, Debug)]
pub struct OpCodeWithPartialStack {
    opcode: OpCode,
    stack_top_items: Vec<U256>,
}

impl Erc7562ValidationTracer {}

impl<DB> Inspector<DB> for Erc7562ValidationTracer
where
    DB: Database,
{
    fn step(&mut self, interp: &mut Interpreter, _context: &mut EvmContext<DB>) {
        let opcode = OpCode::new(interp.current_opcode()).unwrap();

        let mut stack_top_items = vec![];

        for i in 0..=self.config.stack_top_items_size {
            let peeked = interp.stack.peek(i).unwrap();
            stack_top_items.push(peeked);
        }

        let opcode_with_stack = OpCodeWithPartialStack { opcode, stack_top_items };

        // t.handleReturnRevert(opcode)
        // size := len(t.callstackWithOpcodes)
        // currentCallFrame := &t.callstackWithOpcodes[size-1]
        // if t.lastOpWithStack != nil {
        // 	t.handleExtOpcodes(opcode, currentCallFrame)
        // }
        // t.handleAccessedContractSize(opcode, scope, currentCallFrame)
        // if t.lastOpWithStack != nil {
        // 	t.handleGasObserved(opcode, currentCallFrame)
        // }
        // t.storeUsedOpcode(opcode, currentCallFrame)
        // t.handleStorageAccess(opcode, scope, currentCallFrame)
        // t.storeKeccak(opcode, scope)
        // t.lastOpWithStack = opcodeWithStack
    }

    fn step_end(&mut self, _interp: &mut Interpreter, _context: &mut EvmContext<DB>) {
        if self.callstack_with_opcodes.len() != 1 {
            return;
        }
    }

    fn log(
        &mut self,
        _interp: &mut Interpreter,
        _context: &mut EvmContext<DB>,
        log: &alloy_primitives::Log,
    ) {
        if !self.config.with_log {
            return;
        }

        if self.interrupt {
            return;
        }

        // fix unwrap here
        let last = self.callstack_with_opcodes.last_mut().unwrap();

        last.logs.push(CallLog {
            raw_log: log.data.clone(),
            decoded: DecodedCallLog { name: None, params: None },
            position: last.calls.len() as u64,
        })
    }
}
