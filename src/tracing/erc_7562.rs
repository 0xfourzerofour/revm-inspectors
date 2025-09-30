//! Opcount tracing inspector that simply counts all opcodes.
//!
//! See also <https://geth.ethereum.org/docs/developers/evm-tracing/built-in-tracers>

use alloy_primitives::{Address, Bytes, U256};
use alloy_rpc_types_eth::RecoveredAccount;
use alloy_rpc_types_trace::geth::CallLogFrame;
use revm::{
    bytecode::opcode::{CREATE, CREATE2},
    context::ContextTr,
    interpreter::{interpreter::ExtBytecode, interpreter_types::Jumps, Interpreter},
    primitives::{HashMap, HashSet},
    Inspector,
};

#[derive(Default)]
struct AccessedSlots {
    reads: HashMap<Bytes, Vec<Bytes>>,
    writes: HashMap<Bytes, u64>,
    transient_reads: HashMap<Bytes, u64>,
    transient_writes: HashMap<Bytes, u64>,
}

struct ContractSizeWithOpcode {
    contract_size: u64,
    opcode: u8,
}

#[derive(Default)]
struct CallFrameWithOpcodes {
    ty: u8,
    from: Address,
    gas: u64,
    gas_used: u64,
    to: Address,
    input: Bytes,
    output: Bytes,
    error: String,
    revert_reason: String,
    logs: Vec<CallLogFrame>,
    value: U256,
    reverted_snapchat: bool,
    accessed_slots: AccessedSlots,
    ext_code_access_info: Vec<Address>,
    used_opcodes: HashMap<u8, u64>,
    contract_size: HashMap<Address, ContractSizeWithOpcode>,
    out_of_gas: bool,
    keccak_preimages: Vec<Bytes>,
    calls: Vec<CallFrameWithOpcodes>,
}

impl CallFrameWithOpcodes {
    fn process_output(&mut self, output: Bytes, error: String, reverted: bool) {
        self.output = output;
        self.error = error;
        self.reverted_snapchat = reverted;

        if self.ty == CREATE || self.ty == CREATE2 {
            self.to = Address::ZERO;
        }

        // if !errors.Is(err, vm.ErrExecutionReverted) || len(output) == 0 {
        // 	return
        // }

        if self.output.len() < 4 {
            return;
        }

        // f.Output = output
        // if len(output) < 4 {
        // 	return
        // }
        // if unpacked, err := abi.UnpackRevert(output); err == nil {
        // 	f.RevertReason = unpacked
        // }
    }
}

struct OpcodeWithPartialStack {
    opcode: u8,
    stack_top_items: Vec<U256>,
}

struct Erc7562TracerConfig {
    stack_top_items_size: usize,
    ignored_opcodes: Vec<u8>,
    with_log: bool,
}

pub struct Ecr7562Inspector {
    config: Erc7562TracerConfig,
    gas_limit: u64,
    interupt: bool,
    reason: String,
    ignored_opcodes: HashSet<u8>,
    call_stack_with_opcodes: Vec<CallFrameWithOpcodes>,
    last_op_with_stack: OpcodeWithPartialStack,
    keccak_preimages: HashSet<String>,
}

impl Ecr7562Inspector {
    fn capture_end(&mut self, output: Bytes, error: String, reverted: bool) {
        if self.call_stack_with_opcodes.len() != 1 {
            return;
        }

        self.call_stack_with_opcodes[0].process_output(output, error, reverted);
    }
}

impl<CTX> Inspector<CTX> for Ecr7562Inspector
where
    CTX: ContextTr,
{
    fn step(&mut self, interp: &mut Interpreter, context: &mut CTX) {
        if self.interupt {
            return;
        }

        let mut call = CallFrameWithOpcodes {
            ty: interp.bytecode.opcode(),
            from: interp.input.caller_address,
            to: interp.input.target_address,
            input: interp.input.input.bytes(context),
            gas: interp.gas.spent(),
            value: interp.input.call_value,
            ..Default::default()
        };

        if interp.stack.len() == 0 {
            call.gas = self.gas_limit
        }

        self.call_stack_with_opcodes.push(call);
    }
    fn step_end(
        &mut self,
        interp: &mut Interpreter<revm::interpreter::interpreter::EthInterpreter>,
        context: &mut CTX,
    ) {
    }

    fn log(
        &mut self,
        interp: &mut Interpreter<revm::interpreter::interpreter::EthInterpreter>,
        context: &mut CTX,
        log: alloy_primitives::Log,
    ) {
    }
}
