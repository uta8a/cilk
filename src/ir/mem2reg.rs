use crate::ir::{
    basic_block::BasicBlockId,
    function::Function,
    module::Module,
    opcode::{Instruction, InstructionId, Opcode, Operand},
    types::Types,
};
use rustc_hash::FxHashMap;

pub struct Mem2Reg {}

struct Mem2RegOnFunction<'a> {
    cur_func: &'a mut Function,
    inst_indexes: InstructionIndexes,
}

pub type InstructionIndex = usize;

struct InstructionIndexes {
    inst2index: FxHashMap<InstructionId, InstructionIndex>,
}

impl Mem2Reg {
    pub fn new() -> Self {
        Self {}
    }

    pub fn run_on_module(&mut self, module: &mut Module) {
        for (_, func) in &mut module.functions {
            Mem2RegOnFunction {
                cur_func: func,
                inst_indexes: InstructionIndexes::new(),
            }
            .run_on_function(&module.types);
        }
    }
}

impl<'a> Mem2RegOnFunction<'a> {
    fn run_on_function(&mut self, _tys: &Types) {
        let mut single_store_allocas = vec![];
        let mut single_block_allocas = vec![];

        for &id in &self.cur_func.basic_blocks {
            let bb = &self.cur_func.basic_block_arena[id];
            for val in &*bb.iseq.borrow() {
                let inst_id = val.get_inst_id().unwrap();
                let inst = &self.cur_func.inst_table[inst_id];
                if !matches!(inst.opcode, Opcode::Alloca) {
                    continue;
                }
                let alloca = inst;

                let is_promotable = self.is_alloca_promotable(alloca);
                debug!(println!("promotable? {:?}", is_promotable));

                let is_stored_only_once = self.is_alloca_stored_only_once(alloca);
                debug!(println!("single store? {:?}", is_stored_only_once));

                let is_only_used_in_single_block = self.is_alloca_only_used_in_single_block(alloca);
                debug!(println!("single block? {:?}", is_only_used_in_single_block));

                if is_promotable && is_stored_only_once {
                    single_store_allocas.push(inst_id);
                    continue;
                }

                if is_promotable && is_only_used_in_single_block {
                    single_block_allocas.push(inst_id);
                    continue;
                }

                // TODO: support other cases...
            }
        }

        for alloca in single_store_allocas {
            self.promote_single_store_alloca(alloca);
        }

        // for alloca in single_block_allocas {
        //     self.promote_single_block_alloca(alloca);
        // }
    }

    fn is_alloca_promotable(&self, alloca: &Instruction) -> bool {
        // let mut last_parent: Option<BasicBlockId> = None;
        // alloca.users.borrow().iter().all(|&use_id| {
        //     let should_be_load_or_store = &func.inst_table[use_id];
        //     let same_parent = if last_parent.is_some() {
        //         let eq = last_parent.unwrap() == should_be_load_or_store.parent;
        //         last_parent = Some(should_be_load_or_store.parent);
        //         eq
        //     } else {
        //         last_parent = Some(should_be_load_or_store.parent);
        //         true
        //     };
        //     matches!(should_be_load_or_store.opcode, Opcode::Load | Opcode::Store) && same_parent
        // })

        let func = &self.cur_func;
        alloca.users.borrow().iter().all(|&use_id| {
            let should_be_load_or_store = &func.inst_table[use_id];
            matches!(should_be_load_or_store.opcode, Opcode::Load | Opcode::Store)
        })
    }

    fn is_alloca_stored_only_once(&self, alloca: &Instruction) -> bool {
        alloca.users.borrow().iter().fold(0usize, |acc, &use_id| {
            matches!(self.cur_func.inst_table[use_id].opcode, Opcode::Store) as usize + acc
        }) == 1
    }

    fn is_alloca_only_used_in_single_block(&self, alloca: &Instruction) -> bool {
        let mut last_parent: Option<BasicBlockId> = None;
        alloca.users.borrow().iter().all(|&user_id| {
            let user = &self.cur_func.inst_table[user_id];
            let same_parent = last_parent.get_or_insert(user.parent) == &user.parent;
            last_parent = Some(user.parent);
            matches!(user.opcode, Opcode::Load | Opcode::Store) && same_parent
        })
    }

    fn promote_single_store_alloca(&mut self, alloca_id: InstructionId) {
        let alloca = &self.cur_func.inst_table[alloca_id];
        let mut src = None;
        let mut store_to_remove = None;
        let mut loads_to_remove = vec![];

        for &use_id in &*alloca.users.borrow() {
            let inst = &self.cur_func.inst_table[use_id];
            match inst.opcode {
                Opcode::Store => {
                    let store = inst;
                    let store_id = use_id;
                    src = Some(store.operands[0]);
                    store_to_remove = Some(store_id);
                }
                Opcode::Load => {
                    let load = inst;
                    let load_id = use_id;
                    loads_to_remove.push((load_id, load.users.borrow().clone()));
                }
                _ => unreachable!(),
            }
        }

        let src = src.unwrap();
        let store_to_remove = store_to_remove.unwrap();
        let store_idx = self.inst_indexes.get_index(&self.cur_func, store_to_remove);

        // can't handle loads before store so ignore them
        let mut all_loads_removable = true;
        loads_to_remove.retain(|&(id, _)| {
            let load_idx = self.inst_indexes.get_index(&self.cur_func, id);
            let valid = store_idx < load_idx;
            all_loads_removable &= valid;
            valid
        });

        if all_loads_removable {
            self.cur_func.remove_inst(store_to_remove);
            self.cur_func.remove_inst(alloca_id);
        }

        // remove loads and replace them with src
        for (load, users_load) in loads_to_remove {
            self.cur_func.remove_inst(load);

            for u in users_load {
                let inst = &mut self.cur_func.inst_table[u];
                inst.replace_operand(&Operand::new_inst(self.cur_func.id.unwrap(), load), src)
            }
        }
    }

    // fn promote_single_block_alloca(&mut self, alloca_id: InstructionId) {
    // }
}

impl InstructionIndexes {
    pub fn new() -> Self {
        Self {
            inst2index: FxHashMap::default(),
        }
    }

    pub fn get_index(&mut self, cur_func: &Function, inst_id: InstructionId) -> InstructionIndex {
        if let Some(idx) = self.inst2index.get(&inst_id) {
            return *idx;
        }

        let inst = &cur_func.inst_table[inst_id];
        let mut i = 0;
        let bb = &cur_func.basic_block_arena[inst.parent];
        for val in &*bb.iseq_ref() {
            let inst_id = val.as_instruction().id;
            let opcode = cur_func.inst_table[inst_id].opcode;
            if Self::is_interesting_opcode(opcode) {
                self.inst2index.insert(inst_id, i);
            }
            i += 1;
        }

        self.get_index(cur_func, inst_id)
    }

    pub fn is_interesting_opcode(opcode: Opcode) -> bool {
        matches!(opcode, Opcode::Store | Opcode::Load | Opcode::Alloca)
    }
}
