use ahash::HashMap;
use alloy_primitives::{Address, B256, U256};
use reth::{primitives::TransactionSignedEcRecovered, revm::access_list::AccessListInspector};
use revm::{
    interpreter::{opcode, Interpreter},
    Database, EvmContext, Inspector,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SlotKey {
    pub address: Address,
    pub key: B256,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsedStateTrace {
    /// read set contains first read
    pub read_set: HashMap<SlotKey, B256>,
    /// write set contains last write
    pub write_set: HashMap<SlotKey, B256>,
}

#[derive(Debug)]
struct UsedStateEVMInspector<'a> {
    // if previous instruction was sload we store key that was used to call sload here
    tmp_sload_key: Option<B256>,
    used_state_trace: &'a mut UsedStateTrace,
}

impl<'a> UsedStateEVMInspector<'a> {
    fn new(used_state_trace: &'a mut UsedStateTrace) -> Self {
        Self {
            tmp_sload_key: None,
            used_state_trace,
        }
    }

    fn use_tx_nonce(&mut self, tx: &TransactionSignedEcRecovered) {
        self.used_state_trace.read_set.insert(
            SlotKey {
                address: tx.signer(),
                key: Default::default(),
            },
            U256::from(tx.nonce()).into(),
        );
        self.used_state_trace.write_set.insert(
            SlotKey {
                address: tx.signer(),
                key: Default::default(),
            },
            U256::from(tx.nonce() + 1).into(),
        );
    }
}

impl<'a, DB> Inspector<DB> for UsedStateEVMInspector<'a>
where
    DB: Database,
{
    fn step(&mut self, interpreter: &mut Interpreter, _: &mut EvmContext<DB>) {
        if let Some(slot) = self.tmp_sload_key.take() {
            if let Ok(value) = interpreter.stack.peek(0) {
                let value = B256::from(value.to_be_bytes());
                let key = SlotKey {
                    address: interpreter.contract.address,
                    key: slot,
                };
                self.used_state_trace.read_set.entry(key).or_insert(value);
            }
        }

        match interpreter.current_opcode() {
            opcode::SLOAD => {
                if let Ok(slot) = interpreter.stack().peek(0) {
                    let slot = B256::from(slot.to_be_bytes());
                    self.tmp_sload_key = Some(slot);
                }
            }
            opcode::SSTORE => {
                if let (Ok(slot), Ok(value)) =
                    (interpreter.stack().peek(0), interpreter.stack().peek(1))
                {
                    let value = B256::from(value.to_be_bytes());
                    let key = SlotKey {
                        address: interpreter.contract.address,
                        key: B256::from(slot.to_be_bytes()),
                    };
                    self.used_state_trace.write_set.insert(key, value);
                }
            }
            _ => (),
        }
    }
}

#[derive(Debug)]
pub struct RBuilderEVMInspector<'a> {
    access_list_inspector: AccessListInspector,
    used_state_inspector: Option<UsedStateEVMInspector<'a>>,
}

impl<'a> RBuilderEVMInspector<'a> {
    pub fn new(
        tx: &TransactionSignedEcRecovered,
        used_state_trace: Option<&'a mut UsedStateTrace>,
    ) -> Self {
        let access_list_inspector = AccessListInspector::new(
            tx.as_eip2930()
                .map(|tx| tx.access_list.clone())
                .unwrap_or_default(),
            tx.signer(),
            tx.to().unwrap_or_default(),
            None,
        );

        let mut used_state_inspector = used_state_trace.map(UsedStateEVMInspector::new);
        if let Some(i) = &mut used_state_inspector {
            i.use_tx_nonce(tx);
        }

        Self {
            access_list_inspector,
            used_state_inspector,
        }
    }

    pub fn into_access_list(self) -> reth::rpc::types::AccessList {
        self.access_list_inspector.into_access_list()
    }
}

impl<'a, DB> Inspector<DB> for RBuilderEVMInspector<'a>
where
    DB: Database,
{
    #[inline]
    fn step(&mut self, interp: &mut Interpreter, data: &mut EvmContext<DB>) {
        self.access_list_inspector.step(interp, data);
        if let Some(used_state_inspector) = &mut self.used_state_inspector {
            used_state_inspector.step(interp, data);
        }
    }
}