//! ERC 7562 validation tracer.
//!
//! See also <https://geth.ethereum.org/docs/developers/evm-tracing/built-in-tracers>

use std::fmt::Debug;

use alloy_primitives::{
    bytes::Bytes,
    map::foldhash::{HashMap, HashSet, HashSetExt},
    Address, U256,
};
use revm::{
    interpreter::{Interpreter, OpCode},
    Database, EvmContext, Inspector,
};

use super::types::{CallLog, DecodedCallLog};

macro_rules! increment_count {
    ($map:expr, $k:expr) => {{
        $map.entry($k).and_modify(|v| *v += 1).or_insert(1);
    }};
}

#[derive(Clone, Debug, Default)]
pub struct Erc7562ValidationTracer {
    config: Erc7562ValidationTracerConfig,
    gas_limit: u64,
    depth: usize,
    interrupt: bool,
    reason: String,
    // ignoredOpcodes       map[vm.OpCode]struct{}
    callstack_with_opcodes: Vec<CallFrameWithOpCodes>,
    last_opcode_with_stack: Option<OpCodeWithPartialStack>,
    keccak: HashSet<Bytes>,
}

#[derive(Clone, Debug, Default)]
pub struct Erc7562ValidationTracerConfig {
    stack_top_items_size: usize,
    ignored_opcodes: HashSet<OpCode>,
    with_log: bool,
}

impl Erc7562ValidationTracerConfig {
    pub fn new() -> Self {
        Self { stack_top_items_size: 3, with_log: true, ignored_opcodes: default_ignored_opcodes() }
    }
}

#[derive(Clone, Debug)]
struct CallFrameWithOpCodes {
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
struct AccessedSlots {
    reads: HashMap<String, Vec<String>>,
    writes: HashMap<String, u64>,
    transient_reads: HashMap<String, u64>,
    transient_writes: HashMap<String, u64>,
}

#[derive(Clone, Debug)]
struct ContractSizeWithOpCode {
    contract_size: usize,
    opcode: OpCode,
}

#[derive(Clone, Debug)]
struct OpCodeWithPartialStack {
    opcode: OpCode,
    stack_top_items: Vec<U256>,
}

impl Erc7562ValidationTracer {
    pub fn new() -> Self {
        Self { config: Erc7562ValidationTracerConfig::new(), ..Default::default() }
    }

    fn handle_ext_opcodes(
        &mut self,
        opcode: OpCode,
        current_call_frame: &mut CallFrameWithOpCodes,
    ) {
        if let Some(last) = self.last_opcode_with_stack.clone() {
            if is_ext(last.opcode) {
                let addr = Address::from_slice(&last.stack_top_items[0].as_le_slice());

                if !(last.opcode == OpCode::EXTCODESIZE && opcode == OpCode::ISZERO) {
                    current_call_frame.ext_code_access_info.push(addr);
                }
            }
        }
    }

    fn check_revert(&mut self, opcode: OpCode) {
        if opcode == OpCode::REVERT || opcode == OpCode::RETURN {
            self.last_opcode_with_stack = None
        }
    }

    fn handle_gas_observed(
        &mut self,
        opcode: OpCode,
        current_call_frame: &mut CallFrameWithOpCodes,
    ) {
        if let Some(last) = self.last_opcode_with_stack.clone() {
            let pending_gas_observed = last.opcode == OpCode::GAS && !is_call(opcode);
            if pending_gas_observed {
                increment_count!(current_call_frame.used_opcodes, OpCode::GAS);
            }
        }
    }

    fn store_keccak(&mut self, opcode: OpCode, scope: &mut Interpreter) {
        if opcode == OpCode::KECCAK256 {
            let data_offset: u64 = scope.stack.peek(0).unwrap().to();
            let data_length: u64 = scope.stack.peek(1).unwrap().to();
            let memory = &scope.shared_memory;
            let data = memory.slice(data_offset as usize, data_length as usize).to_vec();
            let keccak_bytes = Bytes::from(data);
            self.keccak.insert(keccak_bytes);
        }
    }

    fn store_used_opcode(&mut self, opcode: OpCode, current_call_frame: &mut CallFrameWithOpCodes) {
        if opcode != OpCode::GAS && !self.config.ignored_opcodes.contains(&opcode) {
            increment_count!(current_call_frame.used_opcodes, opcode);
        }
    }

    fn handle_accessed_contract_size<DB: Database>(
        &mut self,
        opcode: OpCode,
        scope: &mut Interpreter,
        context: &mut EvmContext<DB>,
        current_call_frame: &mut CallFrameWithOpCodes,
    ) {
        if is_ext_or_call(opcode) {
            let mut n = 0;
            if !is_ext(opcode) {
                n = 1
            }

            let addr = Address::from_slice(scope.stack.peek(n).unwrap().as_le_slice());
            if !current_call_frame.contract_size.contains_key(&addr) && !is_allowed_precompile(addr)
            {
                if let Ok(code) = context.code(addr) {
                    current_call_frame
                        .contract_size
                        .insert(addr, ContractSizeWithOpCode { contract_size: code.len(), opcode });
                }
            }
        }
    }

    fn handle_storage_access<DB: Database>(
        &mut self,
        opcode: OpCode,
        scope: &mut Interpreter,
        context: &mut EvmContext<DB>,
        current_call_frame: &mut CallFrameWithOpCodes,
    ) {
        if matches!(opcode, OpCode::SLOAD | OpCode::SSTORE | OpCode::TLOAD | OpCode::TSTORE) {
            let slot = scope.stack.peek(0).unwrap();
            let address = Address::from_slice(slot.as_le_slice());
            let slot_hex = format!("{:#x}", slot);

            match opcode {
                OpCode::SLOAD => {
                    let reads = current_call_frame.accessed_slots.reads.get(&slot_hex);
                    let writes = current_call_frame.accessed_slots.reads.get(&slot_hex);

                    if reads.is_none() && writes.is_none() {
                        if let Ok(state) = context.db.storage(address, slot) {
                            current_call_frame
                                .accessed_slots
                                .reads
                                .insert(slot_hex, vec![format!("{:#x}", state)]);
                        }
                    }
                }
                OpCode::SSTORE => {
                    increment_count!(current_call_frame.accessed_slots.writes, slot_hex);
                }
                OpCode::TLOAD => {
                    increment_count!(current_call_frame.accessed_slots.transient_reads, slot_hex);
                }
                _ => {
                    increment_count!(current_call_frame.accessed_slots.transient_writes, slot_hex);
                }
            }
        }
    }
}

impl<DB> Inspector<DB> for Erc7562ValidationTracer
where
    DB: Database,
{
    fn step(&mut self, interp: &mut Interpreter, context: &mut EvmContext<DB>) {
        let opcode = OpCode::new(interp.current_opcode()).unwrap();

        let mut stack_top_items = vec![];

        for i in 0..=self.config.stack_top_items_size {
            let peeked = interp.stack.peek(i).unwrap();
            stack_top_items.push(peeked);
        }

        let opcode_with_stack = OpCodeWithPartialStack { opcode, stack_top_items };

        let mut current_call_frame = self.callstack_with_opcodes.last().unwrap().clone();

        if self.last_opcode_with_stack.is_some() {
            self.handle_ext_opcodes(opcode, &mut current_call_frame);
        }

        self.check_revert(opcode);

        self.handle_accessed_contract_size(opcode, interp, context, &mut current_call_frame);

        if self.last_opcode_with_stack.is_some() {
            self.handle_gas_observed(opcode, &mut current_call_frame);
        }

        self.store_used_opcode(opcode, &mut current_call_frame);
        self.handle_storage_access(opcode, interp, context, &mut current_call_frame);
        self.store_keccak(opcode, interp);
        self.last_opcode_with_stack = Some(opcode_with_stack);
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

        if let Some(last) = self.callstack_with_opcodes.last_mut() {
            last.logs.push(CallLog {
                raw_log: log.data.clone(),
                decoded: DecodedCallLog { name: None, params: None },
                position: last.calls.len() as u64,
            })
        }
    }
}

fn is_ext_or_call(opcode: OpCode) -> bool {
    is_ext(opcode) || is_call(opcode)
}

fn is_ext(opcode: OpCode) -> bool {
    matches!(opcode, OpCode::EXTCODEHASH | OpCode::EXTCODESIZE | OpCode::EXTCODECOPY)
}

fn is_call(opcode: OpCode) -> bool {
    matches!(opcode, OpCode::CALL | OpCode::CALLCODE | OpCode::DELEGATECALL | OpCode::STATICCALL)
}

fn is_allowed_precompile(address: Address) -> bool {
    let address_int = U256::from_le_slice(address.as_slice());
    return address_int > U256::ZERO && address_int < U256::from(10u32);
}

fn default_ignored_opcodes() -> HashSet<OpCode> {
    let mut ignored = HashSet::new();

    // Allow all PUSHx, DUPx, and SWAPx opcodes
    for op in OpCode::PUSH0.get()..=OpCode::SWAP16.get() {
        let op = unsafe { std::mem::transmute(op) };
        ignored.insert(op);
    }

    let additional_ops = [
        OpCode::POP,
        OpCode::ADD,
        OpCode::SUB,
        OpCode::MUL,
        OpCode::DIV,
        OpCode::EQ,
        OpCode::LT,
        OpCode::GT,
        OpCode::SLT,
        OpCode::SGT,
        OpCode::SHL,
        OpCode::SHR,
        OpCode::AND,
        OpCode::OR,
        OpCode::NOT,
        OpCode::ISZERO,
    ];

    for op in additional_ops {
        ignored.insert(op);
    }

    ignored
}
