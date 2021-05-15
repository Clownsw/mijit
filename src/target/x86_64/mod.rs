use super::super::buffer::{Buffer, Mmap};

mod assembler;
pub use assembler::{Assembler, Precision, Register, BinaryOp, ShiftOp, Condition, Width};
use Register::*;

mod lowerer;
pub use lowerer::{Lowerer, ALLOCATABLE_REGISTERS};

/**
 * In the System V amd64 calling convention, these registers must be preserved
 * by subroutines, as must `RSP`.
 */
pub const CALLEE_SAVES: [Register; 6] = [RB, RBP, R12, R13, R14, R15];

/**
 * In the System V amd64 calling convention, these registers may be
 * corrupted by subroutines.
 */
pub const CALLER_SAVES: [Register; 9] = [RDI, RSI, RD, RC, R8, R9, R10, R11, RA];

/**
 * In the System V amd64 calling convention, these registers hold the integer-
 * or pointer-type function arguments.
 */
pub const ARGUMENTS: [Register; 6] = [RDI, RSI, RD, RC, R8, R9];

/**
 * In the System V amd64 calling convention, these registers hold the integer-
 * or pointer-type function results.
 */
pub const RESULTS: [Register; 2] = [RA, RD];

/** The x86_64/libc compilation target. */
pub struct Target;

impl super::Target for Target {
    type Lowerer = Lowerer<Mmap>;

    const NUM_REGISTERS: usize = ALLOCATABLE_REGISTERS.len();

    fn lowerer(&self, capacity: usize) -> Self::Lowerer {
        let buffer = Mmap::new(capacity).expect("Allocation failed");
        Lowerer {a: Assembler::new(buffer)}
    }

    fn execute<T>(
        &self,
        lowerer: Self::Lowerer,
        callback: impl FnOnce(&[u8]) -> T,
    ) -> std::io::Result<(Self::Lowerer, T)> {
        lowerer.use_assembler(|a| {
            a.use_buffer(|b| {
                b.execute(callback)
            })
        })
    }
}