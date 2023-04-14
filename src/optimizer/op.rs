use super::code::{Register, Variable, Precision, UnaryOp, BinaryOp, Width, Action};

/// Annotates a [`Node`] of a [`Dataflow`] graph.
///
/// `Node`: super::Node
/// `Dataflow`: super::Dataflow
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    /// Represents a control-flow decision.
    Guard,
    /// A no-op used at the external boundaries of a Dataflow graph.
    Convention,
    Constant(i64),
    Unary(Precision, UnaryOp),
    Binary(Precision, BinaryOp),
    Load(Width),
    Store(Width),
    Debug,
}

impl Op {
    /// Aggregates this [`Op`] with the specified outputs and inputs to make an
    /// [`Action`].
    /// Panics if the `Op` is a `Guard` or `Convention`.
    pub fn to_action(self, outs: &[Register], ins: &[Variable]) -> Action {
        match self {
            Op::Guard => panic!("Cannot convert a guard to an action"),
            Op::Convention => panic!("Cannot convert a convention to an action"),
            Op::Constant(c) => {
                assert_eq!(outs.len(), 1);
                assert_eq!(ins.len(), 0);
                Action::Constant(Precision::P64, outs[0], c)
            },
            Op::Unary(prec, op) => {
                assert_eq!(outs.len(), 1);
                assert_eq!(ins.len(), 1);
                Action::Unary(op, prec, outs[0], ins[0])
            },
            Op::Binary(prec, op) => {
                assert_eq!(outs.len(), 1);
                assert_eq!(ins.len(), 2);
                Action::Binary(op, prec, outs[0], ins[0], ins[1])
            },
            Op::Load(width) => {
                assert_eq!(outs.len(), 1);
                assert_eq!(ins.len(), 1);
                Action::Load(outs[0], (ins[0], width))
            },
            Op::Store(width) => {
                assert_eq!(outs.len(), 1);
                assert_eq!(ins.len(), 2);
                Action::Store(outs[0], ins[0], (ins[1], width))
            },
            Op::Debug => {
                assert_eq!(outs.len(), 0);
                assert_eq!(ins.len(), 1);
                Action::Debug(ins[0])
            },
        }
    }
}
