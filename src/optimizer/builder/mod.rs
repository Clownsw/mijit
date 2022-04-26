use std::fmt::{Debug};
use std::collections::{HashSet, HashMap};

use super::{code, target, cost, cft, Dataflow, Node, Out, Cold, CFT, Op, Resources, LookupLeaf};
use code::{Slot, Variable, Convention, Ending, EBB};
use crate::util::{ArrayMap};

mod flood;
use flood::{flood};

mod keep_alive;
pub use keep_alive::{keep_alive_sets, HotPathTree, GuardFailure};

mod allocator;
pub use allocator::{Instruction, allocate};

mod moves;
pub use moves::{moves};

mod codegen;
pub use codegen::{CodeGen};

//-----------------------------------------------------------------------------

use code::{Register};

const NUM_REGISTERS: usize = target::x86_64::ALLOCATABLE_REGISTERS.len();

fn all_registers() -> impl Iterator<Item=Register> {
    (0..NUM_REGISTERS).map(|i| Register::new(i as u8).unwrap())
}

//-----------------------------------------------------------------------------

struct Builder<'a> {
    dataflow: &'a Dataflow,
    marks: ArrayMap<Node, usize>,
}

impl<'a> Builder<'a> {
    fn new(dataflow: &'a Dataflow) -> Self {
        let mut marks = dataflow.node_map();
        marks[dataflow.entry_node()] = 1;
        Builder {dataflow, marks}
    }

    /** Tests whether `node` is a [`Op::Guard`]. */
    fn is_guard(&self, node: Node) -> bool {
        matches!(self.dataflow.op(node), Op::Guard)
    }

    /**
     * Converts a [`HotPathTree`] into an [`EBB`]. Optimises the hot path in
     * isolation, and recurses on the cold paths, passing information about
     * [`Variable`] allocation and instruction scheduling.
     *
     * On entry and on exit, `marks[node]` must be in `1..coldness` if
     * `node` is before the guard from which the `HotPathTree` diverges, and
     * `0` otherwise. `marks[dataflow.entry_node()]` must be `1`.
     *
     * - exit - the exit [`Node`] of the hot path.
     * - leaf - the merge point of the hot path.
     * - guard_failure - called for each [`Op::Guard`] to find out what to do
     *   if the guard fails.
     * - coldness - 2 + the number of cold branches needed to reach the
     *   `HotPathTree`. (`0` is used for unmarked nodes, and `1` for
     *   `dataflow.entry_node()`.
     * - slots_used - the number of [`Slot`]s on entry to the `HotPathTree`.
     * - input - called once for each input to the `HotPathTree`. It informs
     *   the caller of `walk()` that the input is live, and returns the
     *   [`Variable`] that holds it.
     */
    pub fn walk<'w, L: Debug + Clone>(
        &'w mut self,
        exit: Node,
        leaf: &'w L,
        guard_failure: &dyn Fn(Node) -> &'w GuardFailure<L>,
        coldness: usize,
        slots_used: usize,
        input: &mut dyn FnMut(Out) -> Variable,
        lookup_leaf: &impl LookupLeaf<L>,
    ) -> EBB<L> {
        println!("\n*** Entering walk({:?}, {:?}) ***", exit, leaf);
        let mut inputs = HashSet::new();
        let mut effects = HashSet::new();
        let nodes = flood(self.dataflow, &mut self.marks, coldness, &mut inputs, &mut effects, exit);
        for &node in &*nodes {
            if self.is_guard(node) {
                for &out in &guard_failure(node).keep_alives {
                    if self.marks[self.dataflow.out(out).0] < coldness {
                        inputs.insert(out);
                    }
                }
            }
        }
        println!("nodes = {:?}", nodes);
        let inputs: Box<[_]> = inputs.into_iter().collect(); // Define an order.
        let input_variables: Box<[_]> = inputs.iter().copied().map(input).collect();
        let mut variables: HashMap<Out, Variable> = inputs.iter().zip(&*input_variables).map(
            |(&out, &variable)| (out, variable)
        ).collect();
        println!("Based on inputs, variables = {:#?}", variables);
        let before: Convention = Convention {slots_used, live_values: input_variables};
        let (instructions, allocation) = allocate(
            &effects,
            &variables,
            self.dataflow,
            &*nodes,
            |node| if self.is_guard(node) { Some(&guard_failure(node).keep_alives) } else { None },
        );
        println!("instructions = {:?}", instructions);
        println!("allocation = {:#?}", allocation);

        // Allocate spill slots on the hot path.
        // Also, find the final location of each `Out`.
        let mut slots_used = before.slots_used;
        for &instruction in &instructions {
            match instruction {
                Instruction::Spill(out1, out2) => {
                    variables.insert(out1, Slot(slots_used).into());
                    slots_used += 1;
                    variables.insert(out2, Slot(slots_used).into());
                    slots_used += 1;
                },
                Instruction::Node(node) => {
                    for out in self.dataflow.outs(node) {
                        let r = allocation[out].expect("Missing destination register");
                        variables.insert(out, r.into());
                    }
                },
            }
        }
        println!("Including instructions, variables = {:#?}", variables);

        let mut cg = CodeGen::new(
            self.dataflow,
            allocation,
            slots_used,
            variables,
            exit,
            lookup_leaf.after(&leaf),
        );

        // Depth-first walk from the exits back to the entry.
        let mut ending = Ending::Leaf(leaf.clone());
        self.marks[exit] = 0;
        for &instruction in instructions.iter().rev() {
            println!("instruction = {:?}", instruction);
            match instruction {
                Instruction::Spill(out1, out2) => {
                    cg.add_spill(out1, out2);
                },
                Instruction::Node(node) => {
                    if self.is_guard(node) {
                        // Make an `EBB` for the hot path.
                        let hot = cg.ebb(ending);
                        // Make `EBB`s for the cold paths path.
                        let gf = guard_failure(node);
                        let cold = gf.cold.map(|child| self.walk(
                            child.exit,
                            &child.leaf,
                            &|guard_node| child.children.get(&guard_node).unwrap_or_else(|| guard_failure(guard_node)),
                            coldness + 1,
                            cg.slots_used(),
                            &mut |out| {
                                println!("nested walk() asked about {:?}", out);
                                cg.read(out)
                            },
                            lookup_leaf,
                        ));
                        // Combine the hot and cold paths and update `ending`.
                        let cft_switch: cft::Switch<EBB<_>> = cold.insert_hot(hot);
                        ending = Ending::Switch(code::Switch::Index {
                            discriminant: {
                                let outs = self.dataflow.ins(cft_switch.guard);
                                assert_eq!(outs.len(), 1);
                                cg.read(outs[0])
                            },
                            cases: cft_switch.cases,
                            default_: cft_switch.default_,
                        });
                    } else {
                        if self.dataflow.cost(node).resources != Resources::new(0) {
                            // The node is not a no-op.
                            cg.add_node(node);
                        }
                    }
                    self.marks[node] = 0;
                },
            }
        }
        println!("*** Leaving walk({:?}, {:?}) ***\n", exit, leaf);
        cg.ebb(ending)
    }
}

pub fn build<L: Debug + Clone>(
    before: &Convention,
    dataflow: &Dataflow,
    cft: &CFT<L>,
    lookup_leaf: &impl LookupLeaf<L>,
) -> EBB<L> {
    println!("dataflow = {:#?}", dataflow);
    // Work out what is where.
    let input_map: HashMap<Out, Variable> =
        dataflow.outs(dataflow.entry_node())
        .zip(&*before.live_values)
        .map(|(out, &variable)| (out, variable))
        .collect();
    println!("input_map = {:#?}", input_map);
    // Compute the keep-alive sets.
    let tree = keep_alive_sets(dataflow, cft);
    println!("tree = {:#?}", tree);
    // Build the new `EBB`.
    let mut builder = Builder::new(dataflow);
    builder.walk(
        tree.exit,
        &tree.leaf,
        &|guard_node| tree.children.get(&guard_node).expect("Missing GuardFailure"),
        2,
        before.slots_used,
        &mut |out| *input_map.get(&out).unwrap(),
        lookup_leaf,
    )
}

//-----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use code::{Register, REGISTERS, Global, Precision, BinaryOp, builder};
    use BinaryOp::*;
    use Precision::*;
    use crate::util::{ArrayMap, AsUsize};

    // Several tests represent leaves as integers.
    impl LookupLeaf<usize> for Convention {
        fn after(&self, _leaf: &usize) -> &Convention {
            self
        }
        fn weight(&self, leaf: &usize) -> usize {
            *leaf
        }
    }

    #[test]
    fn reorder_guards() {
        // Each leaf will return a single `Register`.
        // Weight = register number.
        let afters: ArrayMap<Register, _> = REGISTERS.iter().map(
            |&r| Convention {live_values: Box::new([r.into()]), slots_used: 0}
        ).collect();
        impl LookupLeaf<Register> for ArrayMap<Register, Convention> {
            fn after(&self, leaf: &Register) -> &Convention {
                &self[*leaf]
            }
            fn weight(&self, leaf: &Register) -> usize {
                leaf.as_usize()
            }
        }
        // Make a `before` Convention.
        let before = Convention {
            live_values: Box::new([
                REGISTERS[0].into(),
                REGISTERS[1].into(),
                REGISTERS[2].into(),
                REGISTERS[3].into(),
            ]),
            slots_used: 0,
        };
        // Make a dataflow graph.
        // x_1, x_2, x_3, x_4 are computed in that order,
        // but tested in reverse order.
        let mut df = Dataflow::new(4);
        let entry = df.entry_node();
        let inputs: Vec<Out> = df.outs(entry).collect();
        let x_0 = inputs[0];
        let m_1 = df.add_node(Op::Binary(P64, Mul), &[], &[x_0, x_0], 1);
        let x_1 = df.outs(m_1).next().unwrap();
        let m_2 = df.add_node(Op::Binary(P64,  Mul), &[], &[x_1, x_1], 1);
        let x_2 = df.outs(m_2).next().unwrap();
        let m_3 = df.add_node(Op::Binary(P64, Mul), &[], &[x_2, x_2], 1);
        let x_3 = df.outs(m_3).next().unwrap();
        let m_4 = df.add_node(Op::Binary(P64, Mul), &[], &[x_3, x_3], 1);
        let x_4 = df.outs(m_4).next().unwrap();
        let g_1 = df.add_node(Op::Guard, &[entry], &[x_4], 0);
        let e_1 = df.add_node(Op::Convention, &[g_1], &[inputs[1]], 0);
        let g_2 = df.add_node(Op::Guard, &[entry], &[x_3], 0);
        let e_2 = df.add_node(Op::Convention, &[g_1, g_2], &[inputs[2]], 0);
        let g_3 = df.add_node(Op::Guard, &[entry], &[x_2], 0);
        let e_3 = df.add_node(Op::Convention, &[g_1, g_2, g_3], &[inputs[3]], 0);
        let e_x = df.add_node(Op::Convention, &[g_1, g_2, g_3], &[x_1], 0);
        // Make a CFT.
        let mut cft = CFT::Merge {exit: e_x, leaf: REGISTERS[11]};
        cft = CFT::switch(g_3, [cft], CFT::Merge {exit: e_3, leaf: REGISTERS[3]}, 0);
        cft = CFT::switch(g_2, [cft], CFT::Merge {exit: e_2, leaf: REGISTERS[2]}, 0);
        cft = CFT::switch(g_1, [cft], CFT::Merge {exit: e_1, leaf: REGISTERS[1]}, 0);
        // Call `build()`.
        let _observed = build(&before, &df, &cft, &afters);
        // TODO: Expected output.
    }

    /** Regression test from Bee. */
    #[test]
    fn bee_1() {
        let convention = Convention {slots_used: 0, live_values: Box::new([Variable::Global(Global(0))])};
        // Make an `EBB`.
        let ebb = builder::build(&|b| {
            b.index(
                Global(0),
                Box::new([
                    builder::build(&|mut b| {
                        b.guard(Global(0), false, builder::build(&|b| {
                            b.jump(5)
                        }));
                        b.jump(4)
                    }),
                    builder::build(&|mut b| {
                        b.guard(Global(0), true, builder::build(&|b| {
                            b.jump(3)
                        }));
                        b.jump(2)
                    }),
                ]),
                builder::build(&|b| { b.jump(1) }),
            )
        });
        // Optimize it.
        // inline let _observed = super::super::optimize(&convention, &ebb, &convention);
        let (dataflow, cft) = super::super::simulate(&convention, &ebb, &convention);
        let _observed = build(&convention, &dataflow, &cft, &convention);
        // TODO: Expected output.
    }

    /** Regression test from Bee. */
    #[test]
    fn bee_2() {
        let convention = Convention {
            slots_used: 0,
            live_values: Box::new([
                Variable::Global(Global(0)),
                Variable::Register(REGISTERS[3]),
            ]),
        };
        // Make an `EBB`.
        let ebb = builder::build(&|mut b| {
            b.binary64(Or, REGISTERS[3], Global(0), Global(0));
            b.guard(Global(0), false, builder::build(&|b| { b.jump(2) }));
            b.guard(Global(0), false, builder::build(&|b| { b.jump(3) }));
            b.binary64(And, REGISTERS[3], Global(0), Global(0));
            b.jump(1)
        });
        // Optimize it.
        // inline let _observed = super::super::optimize(&convention, &ebb, &convention);
        let (dataflow, cft) = super::super::simulate(&convention, &ebb, &convention);
        let _observed = build(&convention, &dataflow, &cft, &convention);
        // TODO: Expected output.
    }
}
