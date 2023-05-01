use std::collections::{HashMap, HashSet};
use std::fmt::{self, Debug, Formatter};

use super::{NUM_REGISTERS, all_registers, Resources, Dataflow, Node};
use super::cost::{BUDGET, SPILL_COST, SLOT_COST};
use super::code::{Register, Variable};
use crate::util::{ArrayMap, map_filter_max, Usage};

mod pool;
use pool::{RegisterPool};

mod placer;
use placer::{Time, LEAST as EARLY, Placer};

//-----------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq)]
pub enum Instruction {
    Spill(Node, Node),
    Node(Node),
}

use Instruction::*;

impl Debug for Instruction {
    fn fmt(&self, f: &mut Formatter) -> Result<(), fmt::Error> {
        match *self {
            Spill(out_x, out_y) => write!(f, "({:?}, {:?})", out_x, out_y),
            Node(node) => node.fmt(f),
        }
    }
}

//-----------------------------------------------------------------------------

/// The state of the code generation algorithm. The state is mutated as
/// [`Instruction`]s are added.
#[derive(Debug)]
struct Allocator<'a> {
    /// The dataflow graph.
    dataflow: &'a Dataflow,
    /// The concatenation of the `keep_alive` sets of all [`Node`]s remaining
    /// to be processed. Each call to `add_node()` pops some `Node`s from this.
    usage: Usage<Node>,
    /// The [`Instruction`]s processed so far.
    placer: Placer<Instruction>,
    /// The `Register` allocated for each `Node`'s result, if any.
    allocation: HashMap<Node, Register>,
    /// The `Time` at which each `Node`'s result was last accessed.
    read_times: HashMap<Node, Time>,
    /// The `Time` at which each `Node`'s result became available.
    write_times: HashMap<Node, Time>,
    /// The `Time` at which each `Node` was executed.
    node_times: HashMap<Node, Time>,
    /// The contents of each [`Register`] at the current time.
    regs: ArrayMap<Register, Option<Node>>,
    /// The `Register` allocator state.
    pool: RegisterPool,
}

impl<'a> Allocator<'a> {
    /// Create a new `Allocator`.
    ///
    ///  - effects - [`Node`]s representing side-effects that have already
    ///  occurred.
    ///  - variables - A mapping from the live [`Node`]s to [`Variable`]s.
    ///  - dataflow - The data flow graph.
    ///  - usage - The suggested execution order and usage information.
    pub fn new(
        effects: &HashSet<Node>,
        variables: &HashMap<Node, Variable>,
        dataflow: &'a Dataflow,
        usage: Usage<Node>,
    ) -> Self {
        // Initialize the data structures with the live registers of `variables`.
        let mut dirty = ArrayMap::new(NUM_REGISTERS);
        let mut allocation: HashMap<Node, Register> = HashMap::new();
        let mut write_times: HashMap<Node, Time> = HashMap::new();
        let mut node_times: HashMap<Node, Time> = HashMap::new();
        for &node in effects {
            node_times.insert(node, EARLY);
        }
        let mut regs: ArrayMap<Register, Option<Node>> = ArrayMap::new(NUM_REGISTERS);
        for (&node, &value) in variables.iter() {
            if usage.topmost(&node).is_some() {
                // `node` is alive on entry.
                if let Variable::Register(reg) = value {
                    dirty[reg] = true;
                    regs[reg] = Some(node);
                    allocation.insert(node, reg);
                }
                write_times.insert(node, EARLY);
            }
        }
        // Construct and return.
        let placer = Placer::new();
        let read_times: HashMap<Node, Time> = HashMap::new();
        let pool = RegisterPool::new(dirty);
        Allocator {dataflow, usage, placer, allocation, read_times, write_times, node_times, regs, pool}
    }

    /// Returns the [`Register`] containing `node`, if any.
    fn current_reg(&self, node: Node) -> Option<Register> {
        self.allocation.get(&node).copied().filter(
            |&reg| self.regs[reg] == Some(node)
        )
    }

    /// Pop one [`Node`] from `self.usage`.
    /// Frees its [`Register`], if any, if the `Node` has no remaining uses.
    fn pop_use(&mut self) -> Node {
        let node = self.usage.pop().expect("Incorrect usage information");
        if self.usage.topmost(&node).is_none() {
            if let Some(reg) = self.current_reg(node) {
                self.pool.free(reg);
            }
        }
        node
    }

    /// Record that we accessed `node` at `time` (either reading or writing).
    fn access(&mut self, node: Node, time: Time) {
        self.read_times.entry(node).or_insert(EARLY).max_with(time);
    }

    /// Select a `Register` to spill and free it.
    fn free_a_register(&mut self) -> Register {
        let i = map_filter_max(all_registers(), |reg| {
            self.regs[reg]
                .filter(|_| !self.pool.is_clean(reg))
                .map(|node| std::cmp::Reverse(
                    self.usage.topmost(&node).expect("Dirty register is unused")
                ))
        }).expect("No register is dirty");
        let reg = Register::new(i as u8).unwrap();
        self.pool.free(reg);
        reg
    }

    /// Spills values until at least `num_required` registers are free.
    fn spill_until(&mut self, num_required: usize) {
        while self.pool.num_clean() < num_required {
            let reg_x = self.free_a_register();
            let reg_y = self.free_a_register();
            // Spill the `Register`.
            let out_x = self.regs[reg_x].unwrap();
            let out_y = self.regs[reg_y].unwrap();
            let mut time = self.write_times[&out_x];
            time.max_with(self.write_times[&out_y]);
            self.placer.add_item(Spill(out_x, out_y), SPILL_COST, &mut time);
            self.access(out_x, time);
            self.access(out_y, time);
        }
    }

    /// Called for each [`Node`] in forwards order.
    /// - `num_keep_alives` - the number of `Node`s whose results must survive
    ///   until after `Node` has been executed. This many items will be popped
    ///   from `self.usage`. These `Node`s are often just the inputs of `node`,
    ///   but can also include e.g. values needed by `node`'s cold paths.
    pub fn add_node(&mut self, node: Node, num_keep_alives: usize) {
        let df: &'a Dataflow = self.dataflow;
        let mut time = EARLY; // Earliest time (in cycles) when we can place `node`.
        // Free every input `Register` that won't be used again.
        let keep_alives: Vec<Node> = (0..num_keep_alives).map(|_| self.pop_use()).collect();
        // Spill until we have enough registers to hold the outputs of `node`.
        if df.has_out(node) { self.spill_until(1); }
        // Bump `time` until the dependencies are available.
        for &dep in df.deps(node) {
            time.max_with(self.node_times[&dep]);
        }
        // Bump `time` until the operands are available.
        let ins = df.ins(node);
        let latencies = df.cost(node).input_latencies;
        assert_eq!(ins.len(), latencies.len());
        for (&in_, &latency) in ins.iter().zip(latencies) {
            time.max_with(self.write_times[&in_] + latency as usize);
        }
        // Bump `time` until a destination register is available.
        if df.has_out(node) {
            let reg = self.pool.allocate();
            self.allocation.insert(node, reg);
            if let Some(prev) = self.regs[reg].replace(node) {
                // `reg` was previously used to hold `prev`.
                if let Some(&read_time) = self.read_times.get(&prev) {
                    // `prev` was last accessed at `read_time`.
                    time.max_with(read_time);
                }
            }
            if self.usage.topmost(&node).is_none() {
                // `node` will never be used again. Free `reg` immediately.
                self.pool.free(reg);
            }
        }
        // Bump `time` until the execution resources are available.
        let mut resources = df.cost(node).resources;
        if df.ins(node).iter().any(|&in_| self.current_reg(in_).is_none()) {
            // We can't be sure it's not still in a `Register`; this is a guess.
            resources += SLOT_COST;
        }
        // FIXME: A long series of zero-cost nodes will crash the placer.
        self.placer.add_item(Node(node), resources, &mut time);
        // Record the node's placement.
        self.node_times.insert(node, time);
        // Record when the inputs were used.
        for in_ in keep_alives {
            self.access(in_, time);
        }
        // Record when the output becomes available.
        if let Some(ol) = df.cost(node).output_latency {
            self.access(node, time);
            self.write_times.insert(node, time + ol as usize);
        }
    }

    /// Read the [`Node`]s that are live on exit.
    fn finish(mut self, num_outputs: usize) -> (Vec<Instruction>, HashMap<Node, Register>) {
        for _ in 0..num_outputs { let _ = self.pop_use(); }
        assert_eq!(self.usage.len(), 0);
        assert!(all_registers().all(|reg| self.pool.is_clean(reg)));
        (self.placer.iter().cloned().collect(), self.allocation)
    }
}

/// Choose the execution order and allocate [`Register`]s.
///
/// - effects - [`Node`]s representing side-effects that have already occurred.
/// - variables - the [`Variable`]s passed on entry to the hot path.
/// - dataflow - the dataflow graph.
/// - nodes - the [`Node`]s that need to be executed on the hot path,
///   topologically sorted.
/// - get_keep_alives - for [`Guard`] `Node`s, returns the dataflow
///   dependencies of the cold paths.
/// - outputs - the [`Node`]s that are live on exit.
///
/// Returns:
/// - instructions - the execution order.
/// - allocation - which `Register` holds each `Node`'s result.
///
/// [`Guard`]: super::Op::Guard
pub fn allocate<'a>(
    effects: &HashSet<Node>,
    variables: &HashMap<Node, Variable>,
    dataflow: &Dataflow,
    nodes: &[Node],
    get_keep_alives: impl Fn(Node) -> Option<&'a HashSet<Node>>,
    outputs: &[Node],
) -> (
    Vec<Instruction>,
    HashMap<Node, Register>
) {
    // Reverse `nodes` and compute their inputs.
    let mut usage = Usage::default();
    for &node in outputs { usage.push(node); }
    let mut nodes_rev: Vec<(Node, usize)> = nodes.iter().rev().map(|&node| {
        let mut keep_alives: Vec<Node> = dataflow.ins(node).to_vec();
        if let Some(ins) = get_keep_alives(node) { keep_alives.extend(ins); }
        for &in_ in &keep_alives { usage.push(in_); }
        (node, keep_alives.len())
    }).collect();
    // Schedule and allocate registers for every `Node`.
    let mut a = Allocator::new(effects, variables, dataflow, usage);
    while let Some((node, num_keep_alives)) = nodes_rev.pop() {
        a.add_node(node, num_keep_alives);
    }
    a.finish(outputs.len())
}
