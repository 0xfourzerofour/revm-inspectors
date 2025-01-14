//! ERC 7562 validation tracer.
//!
//! See also <https://geth.ethereum.org/docs/developers/evm-tracing/built-in-tracers>

use std::{collections::HashSet, fmt::Debug};

use alloy_primitives::{bytes::Bytes, Address, U256};
use alloy_rpc_types_trace::geth::{
    erc_7562::{CallFrameWithOpCodes, ContractSizeWithOpCode, Erc7562ValidationTracerConfig},
    CallLogFrame,
};
use alloy_sol_types::RevertReason;
use revm::{
    interpreter::{CallOutcome, CallScheme, InstructionResult, Interpreter, OpCode},
    Database, EvmContext, Inspector,
};

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
    callstack_with_opcodes: Vec<CallFrameWithOpCodes>,
    last_opcode_with_stack: Option<OpCodeWithPartialStack>,
    keccak: HashSet<Bytes>,
}

#[derive(Clone, Debug)]
struct OpCodeWithPartialStack {
    opcode: OpCode,
    stack_top_items: Vec<U256>,
}

fn load_full_config(config: Erc7562ValidationTracerConfig) -> Erc7562ValidationTracerConfig {
    let mut new_config = config.clone();

    if config.ignored_opcodes.is_empty() {
        new_config.ignored_opcodes = default_ignored_opcodes();
    }

    if config.stack_top_items_size == 0 {
        new_config.stack_top_items_size = 3
    }

    new_config
}

impl From<Erc7562ValidationTracerConfig> for Erc7562ValidationTracer {
    fn from(value: Erc7562ValidationTracerConfig) -> Self {
        Self { config: load_full_config(value), ..Default::default() }
    }
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
                let addr = Address::from_word(last.stack_top_items[0].into());
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
                increment_count!(current_call_frame.used_opcodes, OpCode::GAS.get());
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
        if opcode != OpCode::GAS && !self.config.ignored_opcodes.contains(&opcode.get()) {
            increment_count!(current_call_frame.used_opcodes, opcode.get());
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
            let addr = Address::from_word(scope.stack.peek(n).unwrap().into());
            if !current_call_frame.contract_size.contains_key(&addr) && !is_allowed_precompile(addr)
            {
                if let Ok(code) = context.code(addr) {
                    current_call_frame.contract_size.insert(
                        addr,
                        ContractSizeWithOpCode { contract_size: code.len(), opcode: opcode.get() },
                    );
                }
            }
        }
    }

    fn capture_end(&mut self, outcome: CallOutcome) -> CallOutcome {
        if self.callstack_with_opcodes.len() != 1 {
            return outcome;
        }

        self.callstack_with_opcodes[0] =
            process_output(self.callstack_with_opcodes[0].clone(), outcome.clone());

        outcome
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
            let addr = Address::from_word(slot.into());
            let slot_hex = format!("{:#x}", slot);

            match opcode {
                OpCode::SLOAD => {
                    let reads = current_call_frame.accessed_slots.reads.get(&slot_hex);
                    let writes = current_call_frame.accessed_slots.reads.get(&slot_hex);
                    if reads.is_none() && writes.is_none() {
                        if let Ok(state) = context.db.storage(addr, slot) {
                            current_call_frame
                                .accessed_slots
                                .reads
                                .insert(slot_hex, vec![format!("{:#x}", state)]);
                        }
                    }
                }
                OpCode::SSTORE => {
                    println!("HERE SSTORE");
                    increment_count!(current_call_frame.accessed_slots.writes, slot_hex);
                }
                OpCode::TLOAD => {
                    println!("HERE TLOAD");
                    increment_count!(current_call_frame.accessed_slots.transient_reads, slot_hex);
                }
                _ => {
                    println!("HERE OTHER");
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
    fn call(
        &mut self,
        context: &mut EvmContext<DB>,
        inputs: &mut revm::interpreter::CallInputs,
    ) -> Option<revm::interpreter::CallOutcome> {
        self.gas_limit = inputs.gas_limit;
        self.depth = context.journaled_state.depth;

        let mut call = CallFrameWithOpCodes {
            ty: get_opcode_from_call_scheme(inputs.scheme),
            from: inputs.caller,
            to: inputs.target_address,
            input: inputs.input.clone(),
            value: inputs.value.get(),
            ..Default::default()
        };

        if context.journaled_state.depth == 0 {
            call.gas = inputs.gas_limit;
        }

        self.callstack_with_opcodes.push(call);

        None
    }

    fn step(&mut self, interp: &mut Interpreter, context: &mut EvmContext<DB>) {
        let opcode = OpCode::new(interp.current_opcode()).unwrap();

        let mut stack_top_items = vec![];

        for i in 0..=self.config.stack_top_items_size {
            if let Ok(peeked) = interp.stack.peek(i) {
                stack_top_items.push(peeked);
            }
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

    /*

    func (t *erc7562Tracer) OnExit(depth int, output []byte, gasUsed uint64, err error, reverted bool) {
        defer catchPanic()
        if depth == 0 {
            t.captureEnd(output, gasUsed, err, reverted)
            return
        }

        t.depth = depth - 1

        size := len(t.callstackWithOpcodes)
        if size <= 1 {
            return
        }
        // Pop call.
        call := t.callstackWithOpcodes[size-1]
        t.callstackWithOpcodes = t.callstackWithOpcodes[:size-1]
        size -= 1

        if errors.Is(err, vm.ErrCodeStoreOutOfGas) || errors.Is(err, vm.ErrOutOfGas) {
            call.OutOfGas = true
        }
        call.GasUsed = gasUsed
        call.processOutput(output, err, reverted)
        // Nest call into parent.
        t.callstackWithOpcodes[size-1].Calls = append(t.callstackWithOpcodes[size-1].Calls, call)
    }
    */

    fn call_end(
        &mut self,
        _context: &mut EvmContext<DB>,
        _inputs: &revm::interpreter::CallInputs,
        outcome: revm::interpreter::CallOutcome,
    ) -> revm::interpreter::CallOutcome {
        if self.depth == 0 {
            return self.capture_end(outcome);
        }

        self.depth -= 1;

        if self.callstack_with_opcodes.len() <= 1 {
            return outcome;
        }
        let mut call = self.callstack_with_opcodes.pop().unwrap();

        if matches!(
            outcome.result.result,
            InstructionResult::OutOfGas | InstructionResult::OutOfFunds
        ) {
            call.out_of_gas = true;
        }

        call.gas_used = outcome.gas().spent();
        let new_call = process_output(call.clone(), outcome.clone());

        self.callstack_with_opcodes.last_mut().unwrap().calls.push(new_call);

        outcome
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
            last.logs.push(CallLogFrame {
                data: Some(log.data.data.clone()),
                position: Some(last.calls.len() as u64),
                ..Default::default()
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

fn default_ignored_opcodes() -> HashSet<u8> {
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
        ignored.insert(op.get());
    }

    ignored
}

impl From<&Erc7562ValidationTracer> for CallFrameWithOpCodes {
    fn from(value: &Erc7562ValidationTracer) -> Self {
        value.callstack_with_opcodes[0].clone()
    }
}

impl From<Erc7562ValidationTracer> for CallFrameWithOpCodes {
    fn from(value: Erc7562ValidationTracer) -> Self {
        value.callstack_with_opcodes[0].clone()
    }
}

fn get_opcode_from_call_scheme(call_scheme: CallScheme) -> u8 {
    let opcode = match call_scheme {
        CallScheme::Call => OpCode::CALL,
        CallScheme::CallCode => OpCode::CALLCODE,
        CallScheme::DelegateCall => OpCode::DELEGATECALL,
        CallScheme::StaticCall => OpCode::STATICCALL,
        CallScheme::ExtDelegateCall => OpCode::EXTDELEGATECALL,
        CallScheme::ExtStaticCall => OpCode::EXTSTATICCALL,
        CallScheme::ExtCall => OpCode::EXTCALL,
    };

    opcode.get()
}

pub fn process_output(frame: CallFrameWithOpCodes, output: CallOutcome) -> CallFrameWithOpCodes {
    let mut new_frame = frame.clone();

    if output.result.is_ok() {
        new_frame.output = Some(output.result.output);
        return new_frame;
    }

    if output.result.is_error() {
        new_frame.error = format!("{:?}", output.result.result);

        if output.result.output.is_empty() {
            return new_frame;
        }
    }

    if output.result.is_revert() && output.result.output.len() >= 4 {
        if let Some(reason) = RevertReason::decode(&output.result.output) {
            new_frame.revert_reason = reason.to_string();
        }
    }

    new_frame.output = Some(output.result.output);

    new_frame
}
