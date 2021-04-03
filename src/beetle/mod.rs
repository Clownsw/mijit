use std::num::{Wrapping};

use super::code::{
    self, TestOp, Precision, UnaryOp, BinaryOp, Width,
    Action, Case, Slot, Value, IntoValue, Register,
};
use Precision::*;
use UnaryOp::*;
use BinaryOp::*;
use Width::*;
use Action::*;

const TEMP: Register = code::REGISTERS[0];
const R1: Register = code::REGISTERS[1];
const R2: Register = code::REGISTERS[2];
const R3: Register = code::REGISTERS[3];
const R4: Register = code::REGISTERS[4];
const R5: Register = code::REGISTERS[5];

/** Beetle's registers and other globals. */
#[repr(u8)]
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum Global {
    BEP = 0,
    BA = 1,
    BSP = 2,
    BRP = 3,
    BS0 = 4,
    BR0 = 5,
    BThrow = 6,
    BBad = 7,
    BNotAddress = 8,
    BMemory = 9,
    Opcode = 10,
    Stack0 = 11,
    Stack1 = 12,
    LoopFlag = 13,
    LoopStep = 14,
    LoopNew = 15,
    LoopOld = 16,
}

impl From<Global> for Value {
    fn from(r: Global) -> Self {
        Value::Slot(Slot(r as usize))
    }
}

use Global::*;

const NUM_GLOBALS: usize = 17;

/** Beetle's registers. Only these values are live in State::Root. */
pub const ALL_REGISTERS: [Global; 10] = [
    BEP, BA, BSP, BRP, BS0, BR0,
    BThrow, BBad, BNotAddress, BMemory,
];

/** Beetle's address space is unified, so we always use the same AliasMask. */
const MEMORY: code::AliasMask = code::AliasMask(0x1);

/** Computes the number of bytes in `n` cells. */
pub const fn cell_bytes(n: i64) -> i64 { Wrapping(4 * n).0 }

/** The number of bits in a word. */
pub const CELL_BITS: i64 = cell_bytes(8);

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum State {
    Root,
    Dispatch,
    Next,
    Pick,
    Roll,
    Qdup,
    Lshift,
    Rshift,
    Branch,
    Branchi,
    Qbranch,
    Qbranchi,
    Loop,
    Loopi,
    Ploopp,
    Ploopm,
    Ploop,
    Ploopip,
    Ploopim,
    Ploopi,
    Halt,
}

//-----------------------------------------------------------------------------

fn opcode(c: u8) -> TestOp { TestOp::Bits(Opcode.into(), 0xFF, c as i32) }
fn lt(v: impl IntoValue, c: i32) -> TestOp { TestOp::Lt(v.into(), c) }
fn ge(v: impl IntoValue, c: i32) -> TestOp { TestOp::Ge(v.into(), c) }
fn ult(v: impl IntoValue, c: i32) -> TestOp { TestOp::Ult(v.into(), c) }
fn uge(v: impl IntoValue, c: i32) -> TestOp { TestOp::Uge(v.into(), c) }
fn eq(v: impl IntoValue, c: i32) -> TestOp { TestOp::Eq(v.into(), c) }
fn ne(v: impl IntoValue, c: i32) -> TestOp { TestOp::Ne(v.into(), c) }

/** Build a case, in the form that `Beetle::get_code()` returns. */
fn build(
    test_op: TestOp,
    callback: impl FnOnce(&mut Builder),
    state: State,
) -> Case<State> {
    let mut b = Builder::new();
    callback(&mut b);
    Case {condition: (test_op, P32), actions: b.0, new_state: state}
}

/**
 * A utility for generating action routines.
 *
 * The methods correspond roughly to the cases of type Action. They fill in
 * Beetle-specific default parameters. `load()` and `store()` add code to map
 * Beetle addresses to native addresses. `push()` and `pop()` access Beetle
 * stacks (the native stack is not used).
 */
struct Builder(Vec<Action>);

impl Builder {
    fn new() -> Self {
        Builder(Vec::new())
    }

    fn move_(&mut self, dest: impl IntoValue, src: impl IntoValue) {
        self.0.push(Move(dest.into(), src.into()));
    }

    fn const_(&mut self, dest: impl IntoValue, constant: i64) {
        self.0.push(Constant(P32, TEMP, constant));
        self.move_(dest, TEMP);
    }

    /**
     * Apply 32-bit `op` to `src`, writing `dest`.
     * `TEMP` is corrupted.
     */
    fn unary(&mut self, op: UnaryOp, dest: impl IntoValue, src: impl IntoValue) {
        self.0.push(Unary(op, P32, TEMP, src.into()));
        self.move_(dest, TEMP);
    }

    /**
     * Apply 32-bit `op` to `src1` and `src2`, writing `dest`.
     * `TEMP` is corrupted.
     */
    fn binary(&mut self, op: BinaryOp, dest: impl IntoValue, src1: impl IntoValue, src2: impl IntoValue) {
        self.0.push(Binary(op, P32, TEMP, src1.into(), src2.into()));
        self.move_(dest, TEMP);
    }

    /**
     * Apply 32-bit `op` to `src` and `constant`, writing `dest`.
     * `TEMP` is corrupted.
     */
    fn const_binary(&mut self, op: BinaryOp, dest: impl IntoValue, src: impl IntoValue, constant: i64) {
        assert_ne!(src.into(), TEMP.into());
        self.0.push(Constant(P32, TEMP, constant));
        self.binary(op, dest, src, TEMP);
    }

    /**
     * Compute the native address corresponding to `addr`.
     */
    fn native_address(&mut self, dest: Register, addr: impl IntoValue) {
        self.0.push(Binary(Add, P64, dest, BMemory.into(), addr.into()));
    }

    /**
     * Compute the native address corresponding to `addr`, and load 32 bits.
     * `TEMP` is corrupted.
     */
    // TODO: Bounds checking.
    fn load(&mut self, dest: impl IntoValue, addr: impl IntoValue) {
        self.native_address(TEMP, addr);
        self.0.push(Load(TEMP, (TEMP.into(), Four), MEMORY));
        self.move_(dest, TEMP);
    }

    /**
     * Compute the native address corresponding to `addr`, and store 32 bits.
     * `TEMP` is corrupted.
     */
    // TODO: Bounds checking.
    fn store(&mut self, src: impl IntoValue, addr: impl IntoValue) {
        assert_ne!(src.into(), TEMP.into());
        self.native_address(TEMP, addr);
        self.0.push(Store(TEMP, src.into(), (TEMP.into(), Four), MEMORY));
    }

    /**
     * Compute the native address corresponding to `addr`, and load 8 bits.
     * `TEMP` is corrupted.
     */
    // TODO: Bounds checking.
    fn load_byte(&mut self, dest: impl IntoValue, addr: impl IntoValue) {
        self.native_address(TEMP, addr);
        self.0.push(Load(TEMP, (TEMP.into(), One), MEMORY));
        self.move_(dest, TEMP);
    }

    /**
     * Compute the native address corresponding to `addr`, and store 8 bits.
     * `TEMP` is corrupted.
     */
    // TODO: Bounds checking.
    fn store_byte(&mut self, src: impl IntoValue, addr: impl IntoValue) {
        assert_ne!(src.into(), TEMP.into());
        self.native_address(TEMP, addr);
        self.0.push(Store(TEMP, src.into(), (TEMP.into(), One), MEMORY));
    }

    /**
     * `load()` `dest` from `addr`, then increment `addr`.
     * `TEMP` is corrupted.
     */
    fn pop(&mut self, dest: impl IntoValue, addr: impl IntoValue) {
        assert_ne!(dest.into(), addr.into());
        assert_ne!(dest.into(), TEMP.into());
        self.load(dest, addr);
        self.const_binary(Add, TEMP, addr, cell_bytes(1));
        self.move_(addr, TEMP);
    }

    /**
     * Decrement `addr` by `cell_bytes(1)`, then `store()` `src` at `addr`.
     * `TEMP` is corrupted.
     */
    fn push(&mut self, src: impl IntoValue, addr: impl IntoValue) {
        assert_ne!(src.into(), TEMP.into());
        assert_ne!(src.into(), addr.into());
        self.const_binary(Sub, TEMP, addr, cell_bytes(1));
        self.move_(addr, TEMP);
        self.store(src, TEMP);
    }

    #[allow(dead_code)]
    fn debug(&mut self, x: impl IntoValue) {
        self.0.push(Debug(x.into()));
    }
}

//-----------------------------------------------------------------------------

#[derive(Debug)]
pub struct Machine;

impl code::Machine for Machine {
    type State = State;

    fn values(&self) -> Vec<Value> {
        (0..NUM_GLOBALS).map(|i| Slot(i).into()).collect()
    }

    fn get_code(&self, state: Self::State) -> (u64, Vec<Case<Self::State>>) {
        let mut register_mask = 0;
        for &r in &ALL_REGISTERS {
            register_mask |= 1 << r as usize;
        }
        // FIXME: Correct the register masks.
        match state {
            State::Root => (
                register_mask,
                vec![
                    build(TestOp::Always, |b| {
                        b.move_(Opcode, BA);
                        b.const_binary(Asr, BA, BA, 8);
                    }, State::Dispatch),
                ],
            ),
            State::Next => (
                register_mask,
                vec![
                    build(TestOp::Always, |b| {
                        b.pop(BA, BEP);
                    }, State::Root),
                ],
            ),
            State::Pick => {
                let mut pick = Vec::new();
                for u in 0..4 {
                    pick.push(build(eq(Stack0, u), |b| {
                        b.const_binary(Add, R2, BSP, cell_bytes(u as i64 + 1));
                        b.load(R2, R2);
                        b.store(R2, BSP);
                    }, State::Root));
                }
                (register_mask, pick)
            },
            State::Roll => {
                let mut roll = Vec::new();
                for u in 0..4 {
                    roll.push(build(eq(Stack0, u as i32), |b| {
                        b.const_binary(Add, R5, BSP, cell_bytes(u));
                        b.load(R3, R5);
                        for v in 0..u {
                            b.const_binary(Add, R4, BSP, cell_bytes(v));
                            b.load(R2, R4);
                            b.store(R3, R4);
                            b.move_(R3, R2);
                        }
                        b.store(R3, R5);
                    }, State::Root));
                }
                (register_mask, roll)
            },
            State::Qdup => (
                register_mask,
                vec![
                    build(eq(Stack0, 0), |_| {}, State::Root),
                    build(ne(Stack0, 0), |b| {
                        b.push(Stack0, BSP);
                    }, State::Root),
                ],
            ),
            State::Lshift => (
                register_mask,
                vec![
                    build(ult(Stack1, CELL_BITS as i32), |b| {
                        b.binary(Lsl, R2, Stack0, Stack1);
                        b.store(R2, BSP);
                    }, State::Root),
                    build(uge(Stack1, CELL_BITS as i32), |b| {
                        b.const_(R2, 0);
                        b.store(R2, BSP);
                    }, State::Root),
                ],
            ),
            State::Rshift => (
                register_mask,
                vec![
                    build(ult(Stack1, CELL_BITS as i32), |b| {
                        b.binary(Lsr, R2, Stack0, Stack1);
                        b.store(R2, BSP);
                    }, State::Root),
                    build(uge(Stack1, CELL_BITS as i32), |b| {
                        b.const_(R2, 0);
                        b.store(R2, BSP);
                    }, State::Root),
                ],
            ),
            State::Branch => (
                register_mask,
                vec![
                    build(TestOp::Always, |b| {
                        // Load EP from the cell it points to.
                        b.load(BEP, BEP); // FIXME: Add check that EP is valid.
                    }, State::Next),
                ],
            ),
            State::Branchi => (
                register_mask,
                vec![
                    build(TestOp::Always, |b| {
                        b.const_binary(Mul, R2, BA, cell_bytes(1));
                        b.binary(Add, BEP, BEP, R2); // FIXME: Add check that EP is valid.
                    }, State::Next),
                ],
            ),
            State::Qbranch => (
                register_mask,
                vec![
                    build(eq(Stack0, 0), |_| {}, State::Branch),
                    build(ne(Stack0, 0), |b| {
                        b.const_binary(Add, BEP, BEP, cell_bytes(1));
                    }, State::Root),
                ],
            ),
            State::Qbranchi => (
                register_mask,
                vec![
                    build(eq(Stack0, 0), |_| {}, State::Branchi),
                    build(ne(Stack0, 0), |_| {}, State::Next),
                ],
            ),
            State::Loop => (
                register_mask,
                vec![
                    build(eq(LoopFlag, 0), |b| {
                        // Discard the loop index and limit.
                        b.const_binary(Add, BRP, BRP, cell_bytes(2));
                        // Add 4 to EP.
                        b.const_binary(Add, BEP, BEP, cell_bytes(1)); // FIXME: Add check that EP is valid.
                    }, State::Root),
                    build(ne(LoopFlag, 0), |_| {}, State::Branch),
                ],
            ),
            State::Loopi => (
                register_mask,
                vec![
                    build(eq(LoopFlag, 0), |b| {
                        // Discard the loop index and limit.
                        b.const_binary(Add, BRP, BRP, cell_bytes(2));
                    }, State::Next),
                    build(ne(LoopFlag, 0), |_| {}, State::Branchi),
                ],
            ),
            State::Ploopp => (
                register_mask,
                vec![
                    build(lt(LoopFlag, 0), |b| {
                        // Discard the loop index and limit.
                        b.const_binary(Add, BRP, BRP, cell_bytes(2));
                        // Add 4 to EP.
                        b.const_binary(Add, BEP, BEP, cell_bytes(1)); // FIXME: Add check that EP is valid.
                    }, State::Root),
                    build(ge(LoopFlag, 0), |_| {}, State::Branch),
                ],
            ),
            State::Ploopm => (
                register_mask,
                vec![
                    build(lt(LoopFlag, 0), |b| {
                        // Discard the loop index and limit.
                        b.const_binary(Add, BRP, BRP, cell_bytes(2));
                        // Add 4 to EP.
                        b.const_binary(Add, BEP, BEP, cell_bytes(1)); // FIXME: Add check that EP is valid.
                    }, State::Root),
                    build(ge(LoopFlag, 0), |_| {}, State::Branch),
                ],
            ),
            State::Ploop => (
                register_mask,
                vec![
                    build(ge(LoopStep, 0), |b| {
                        b.unary(Not, LoopNew, LoopNew);
                        b.binary(And, LoopNew, LoopNew, LoopOld);
                    }, State::Ploopp),
                    build(lt(LoopStep, 0), |b| {
                        b.unary(Not, LoopOld, LoopOld);
                        b.binary(And, LoopNew, LoopNew, LoopOld);
                    }, State::Ploopm),
                ],
            ),
            State::Ploopip => (
                register_mask,
                vec![
                    build(lt(LoopFlag, 0), |b| {
                        // Discard the loop index and limit.
                        b.const_binary(Add, BRP, BRP, cell_bytes(2));
                    }, State::Root),
                    build(ge(LoopFlag, 0), |_| {}, State::Branchi),
                ],
            ),
            State::Ploopim => (
                register_mask,
                vec![
                    build(lt(LoopFlag, 0), |b| {
                        // Discard the loop index and limit.
                        b.const_binary(Add, BRP, BRP, cell_bytes(2));
                    }, State::Root),
                    build(ge(LoopFlag, 0), |_| {}, State::Branchi),
                ],
            ),
            State::Ploopi => (
                register_mask,
                vec![
                    build(ge(R2, 0), |b| {
                        b.unary(Not, LoopNew, LoopNew);
                        b.binary(And, LoopNew, LoopNew, LoopOld);
                    }, State::Ploopip),
                    build(lt(R2, 0), |b| {
                        b.unary(Not, LoopOld, LoopOld);
                        b.binary(And, LoopNew, LoopNew, LoopOld);
                    }, State::Ploopim),
                ],
            ),
            State::Halt => (register_mask, vec![]),
            State::Dispatch => (
                register_mask,
                vec![
                    // NEXT
                    build(opcode(0x00), |_| {}, State::Next),

                    // DUP
                    build(opcode(0x01), |b| {
                        b.load(R2, BSP);
                        b.push(R2, BSP);
                    }, State::Root),

                    // DROP
                    build(opcode(0x02), |b| {
                        b.const_binary(Add, BSP, BSP, cell_bytes(1));
                    }, State::Root),

                    // SWAP
                    build(opcode(0x03), |b| {
                        b.pop(R4, BSP);
                        b.load(R3, BSP);
                        b.store(R4, BSP);
                        b.push(R3, BSP);
                    }, State::Root),

                    // OVER
                    build(opcode(0x04), |b| {
                        b.const_binary(Add, R2, BSP, cell_bytes(1));
                        b.load(R3, R2);
                        b.push(R3, BSP);
                    }, State::Root),

                    // ROT
                    build(opcode(0x05), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Add, R5, BSP, cell_bytes(1));
                        b.load(R3, R5);
                        b.store(R2, R5);
                        b.const_binary(Add, R5, BSP, cell_bytes(2));
                        b.load(R2, R5);
                        b.store(R3, R5);
                        b.store(R2, BSP);
                    }, State::Root),

                    // -ROT
                    build(opcode(0x06), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Add, R5, BSP, cell_bytes(2));
                        b.load(R3, R5);
                        b.store(R2, R5);
                        b.const_binary(Add, R5, BSP, cell_bytes(1));
                        b.load(R2, R5);
                        b.store(R3, R5);
                        b.store(R2, BSP);
                    }, State::Root),

                    // TUCK
                    build(opcode(0x07), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Add, R5, BSP, cell_bytes(1));
                        b.load(R3, R5);
                        b.store(R2, R5);
                        b.store(R3, BSP);
                        b.push(R2, BSP);
                    }, State::Root),

                    // NIP
                    build(opcode(0x08), |b| {
                        b.pop(R2, BSP);
                        b.store(R2, BSP);
                    }, State::Root),

                    // PICK
                    build(opcode(0x09), |b| {
                        b.load(Stack0, BSP);
                    }, State::Pick),

                    // ROLL
                    build(opcode(0x0a), |b| {
                        b.pop(Stack0, BSP);
                    }, State::Roll),

                    // ?DUP
                    build(opcode(0x0b), |b| {
                        b.load(Stack0, BSP);
                    }, State::Qdup),

                    // >R
                    build(opcode(0x0c), |b| {
                        b.pop(R2, BSP);
                        b.push(R2, BRP);
                    }, State::Root),

                    // R>
                    build(opcode(0x0d), |b| {
                        b.pop(R2, BRP);
                        b.push(R2, BSP);
                    }, State::Root),

                    // R@
                    build(opcode(0x0e), |b| {
                        b.load(R2, BRP);
                        b.push(R2, BSP);
                    }, State::Root),

                    // <
                    build(opcode(0x0f), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Lt, R2, R4, R2);
                        b.store(R2, BSP);
                    }, State::Root),

                    // >
                    build(opcode(0x10), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Lt, R2, R2, R4);
                        b.store(R2, BSP);
                    }, State::Root),

                    // =
                    build(opcode(0x11), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Eq, R2, R2, R4);
                        b.store(R2, BSP);
                    }, State::Root),

                    // <>
                    build(opcode(0x12), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Eq, R2, R2, R4);
                        b.unary(Not, R2, R2);
                        b.store(R2, BSP);
                    }, State::Root),

                    // 0<
                    build(opcode(0x13), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Lt, R2, R2, 0);
                        b.store(R2, BSP);
                    }, State::Root),

                    // 0>
                    build(opcode(0x14), |b| {
                        b.load(R2, BSP);
                        b.const_(R4, 0);
                        b.binary(Lt, R2, R4, R2);
                        b.store(R2, BSP);
                    }, State::Root),

                    // 0=
                    build(opcode(0x15), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Eq, R2, R2, 0);
                        b.store(R2, BSP);
                    }, State::Root),

                    // 0<>
                    build(opcode(0x16), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Eq, R2, R2, 0);
                        b.unary(Not, R2, R2);
                        b.store(R2, BSP);
                    }, State::Root),

                    // U<
                    build(opcode(0x17), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Ult, R2, R4, R2);
                        b.store(R2, BSP);
                    }, State::Root),

                    // U>
                    build(opcode(0x18), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Ult, R2, R2, R4);
                        b.store(R2, BSP);
                    }, State::Root),

                    // 0
                    build(opcode(0x19), |b| {
                        b.const_(R4, 0);
                        b.push(R4, BSP);
                    }, State::Root),

                    // 1
                    build(opcode(0x1a), |b| {
                        b.const_(R4, 1);
                        b.push(R4, BSP);
                    }, State::Root),

                    // -1
                    build(opcode(0x1b), |b| {
                        b.const_(R4, -1);
                        b.push(R4, BSP);
                    }, State::Root),

                    // CELL
                    build(opcode(0x1c), |b| {
                        b.const_(R4, cell_bytes(1));
                        b.push(R4, BSP);
                    }, State::Root),

                    // -CELL
                    build(opcode(0x1d), |b| {
                        b.const_(R4, (-Wrapping(cell_bytes(1))).0);
                        b.push(R4, BSP);
                    }, State::Root),

                    // +
                    build(opcode(0x1e), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Add, R2, R2, R4);
                        b.store(R2, BSP);
                    }, State::Root),

                    // -
                    build(opcode(0x1f), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Sub, R2, R2, R4);
                        b.store(R2, BSP);
                    }, State::Root),

                    // >-<
                    build(opcode(0x20), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Sub, R2, R4, R2);
                        b.store(R2, BSP);
                    }, State::Root),

                    // 1+
                    build(opcode(0x21), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Add, R2, R2, 1);
                        b.store(R2, BSP);
                    }, State::Root),

                    // 1-
                    build(opcode(0x22), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Sub, R2, R2, 1);
                        b.store(R2, BSP);
                    }, State::Root),

                    // CELL+
                    build(opcode(0x23), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Add, R2, R2, cell_bytes(1));
                        b.store(R2, BSP);
                    }, State::Root),

                    // CELL-
                    build(opcode(0x24), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Sub, R2, R2, cell_bytes(1));
                        b.store(R2, BSP);
                    }, State::Root),

                    // *
                    build(opcode(0x25), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Mul, R2, R2, R4);
                        b.store(R2, BSP);
                    }, State::Root),

                    /* TODO:
                    // /
                    build(opcode(0x26), |_| {
                        // TODO
                    }, State::Root),

                    // MOD
                    build(opcode(0x27), |_| {
                        // TODO
                    }, State::Root),

                    // /MOD
                    build(opcode(0x28), |_| {
                        // TODO
                    }, State::Root),

                    // U/MOD
                    build(opcode(0x29), |b| {
                        b.pop(R2, BSP);
                        b.load(R1, BSP);
                        b.0.push(Division(UnsignedDivMod, P32, R1, R2, R1, R2));
                        b.store(R2, BSP);
                        b.push(R1, BSP);
                    }, State::Root),

                    // S/REM
                    build(opcode(0x2a), |b| {
                        b.pop(R2, BSP);
                        b.load(R1, BSP);
                        b.0.push(Division(SignedDivMod, P32, R1, R2, R1, R2));
                        b.store(R2, BSP);
                        b.push(R1, BSP);
                    }, State::Root),
                    */

                    // 2/
                    build(opcode(0x2b), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Asr, R2, R2, 1);
                        b.store(R2, BSP);
                    }, State::Root),

                    // CELLS
                    build(opcode(0x2c), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Mul, R2, R2, cell_bytes(1));
                        b.store(R2, BSP);
                    }, State::Root),

                    // ABS
                    build(opcode(0x2d), |b| {
                        b.load(R2, BSP);
                        b.unary(Abs, R2, R2);
                        b.store(R2, BSP);
                    }, State::Root),

                    // NEGATE
                    build(opcode(0x2e), |b| {
                        b.load(R2, BSP);
                        b.unary(Negate, R2, R2);
                        b.store(R2, BSP);
                    }, State::Root),

                    // MAX
                    build(opcode(0x2f), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Max, R2, R2, R4);
                        b.store(R2, BSP);
                    }, State::Root),

                    // MIN
                    build(opcode(0x30), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Min, R2, R2, R4);
                        b.store(R2, BSP);
                    }, State::Root),

                    // INVERT
                    build(opcode(0x31), |b| {
                        b.load(R2, BSP);
                        b.unary(Not, R2, R2);
                        b.store(R2, BSP);
                    }, State::Root),

                    // AND
                    build(opcode(0x32), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(And, R2, R2, R4);
                        b.store(R2, BSP);
                    }, State::Root),

                    // OR
                    build(opcode(0x33), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Or, R2, R2, R4);
                        b.store(R2, BSP);
                    }, State::Root),

                    // XOR
                    build(opcode(0x34), |b| {
                        b.pop(R2, BSP);
                        b.load(R4, BSP);
                        b.binary(Xor, R2, R2, R4);
                        b.store(R2, BSP);
                    }, State::Root),

                    // LSHIFT
                    build(opcode(0x35), |b| {
                        b.pop(Stack0, BSP);
                        b.load(Stack1, BSP);
                    }, State::Lshift),

                    // RSHIFT
                    build(opcode(0x36), |b| {
                        b.pop(Stack0, BSP);
                        b.load(Stack1, BSP);
                    }, State::Rshift),

                    // 1LSHIFT
                    build(opcode(0x37), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Lsl, R2, R2, 1);
                        b.store(R2, BSP);
                    }, State::Root),

                    // 1RSHIFT
                    build(opcode(0x38), |b| {
                        b.load(R2, BSP);
                        b.const_binary(Lsr, R2, R2, 1);
                        b.store(R2, BSP);
                    }, State::Root),

                    // @
                    build(opcode(0x39), |b| {
                        b.load(R2, BSP);
                        b.load(R2, R2);
                        b.store(R2, BSP);
                    }, State::Root),

                    // !
                    build(opcode(0x3a), |b| {
                        b.pop(R2, BSP);
                        b.pop(R3, BSP);
                        b.store(R3, R2);
                    }, State::Root),

                    // C@
                    build(opcode(0x3b), |b| {
                        b.load(R2, BSP);
                        b.load_byte(R2, R2);
                        b.store(R2, BSP);
                    }, State::Root),

                    // C!
                    build(opcode(0x3c), |b| {
                        b.pop(R2, BSP);
                        b.pop(R3, BSP);
                        b.store_byte(R3, R2);
                    }, State::Root),

                    // +!
                    build(opcode(0x3d), |b| {
                        b.pop(R2, BSP);
                        b.pop(R3, BSP);
                        b.load(R5, R2);
                        b.binary(Add, R3, R5, R3);
                        b.store(R3, R2);
                    }, State::Root),

                    // BSP@
                    build(opcode(0x3e), |b| {
                        b.move_(R1, BSP);
                        b.push(R1, BSP);
                    }, State::Root),

                    // BSP!
                    build(opcode(0x3f), |b| {
                        b.load(BSP, BSP);
                    }, State::Root),

                    // BRP@
                    build(opcode(0x40), |b| {
                        b.push(BRP, BSP);
                    }, State::Root),

                    // BRP!
                    build(opcode(0x41), |b| {
                        b.pop(BRP, BSP);
                    }, State::Root),

                    // EP@
                    build(opcode(0x42), |b| {
                        b.push(BEP, BSP);
                    }, State::Root),

                    // BS0@
                    build(opcode(0x43), |b| {
                        b.push(BS0, BSP);
                    }, State::Root),

                    // BS0!
                    build(opcode(0x44), |b| {
                        b.pop(BS0, BSP);
                    }, State::Root),

                    // BR0@
                    build(opcode(0x45), |b| {
                        b.push(BR0, BSP);
                    }, State::Root),

                    // BR0!
                    build(opcode(0x46), |b| {
                        b.pop(BR0, BSP);
                    }, State::Root),

                    // 'THROW@
                    build(opcode(0x47), |b| {
                        b.push(BThrow, BSP);
                    }, State::Root),

                    // 'THROW!
                    build(opcode(0x48), |b| {
                        b.pop(BThrow, BSP);
                    }, State::Root),

                    // MEMORY@
                    build(opcode(0x49), |b| {
                        b.push(BMemory, BSP);
                    }, State::Root),

                    // 'BAD@
                    build(opcode(0x4a), |b| {
                        b.push(BBad, BSP);
                    }, State::Root),

                    // -ADDRESS@
                    build(opcode(0x4b), |b| {
                        b.push(BNotAddress, BSP);
                    }, State::Root),

                    // BRANCH
                    build(opcode(0x4c), |_| {}, State::Branch),

                    // BRANCHI
                    build(opcode(0x4d), |_| {}, State::Branchi),

                    // ?BRANCH
                    build(opcode(0x4e), |b| {
                        b.pop(Stack0, BSP);
                    }, State::Qbranch),

                    // ?BRANCHI
                    build(opcode(0x4f), |b| {
                        b.pop(Stack0, BSP);
                    }, State::Qbranchi),

                    // EXECUTE
                    build(opcode(0x50), |b| {
                        b.push(BEP, BRP);
                        b.pop(BEP, BSP); // FIXME: Add check that EP is valid.
                    }, State::Next),

                    // @EXECUTE
                    build(opcode(0x51), |b| {
                        b.push(BEP, BRP);
                        b.pop(R1, BSP);
                        b.load(BEP, R1); // FIXME: Add check that EP is valid.
                    }, State::Next),

                    // CALL
                    build(opcode(0x52), |b| {
                        b.const_binary(Add, R1, BEP, cell_bytes(1));
                        b.push(R1, BRP);
                    }, State::Branch),

                    // CALLI
                    build(opcode(0x53), |b| {
                        b.push(BEP, BRP);
                    }, State::Branchi),

                    // EXIT
                    build(opcode(0x54), |b| {
                        b.pop(BEP, BRP); // FIXME: Add check that EP is valid.
                    }, State::Next),

                    // (DO)
                    build(opcode(0x55), |b| {
                        // Pop two items from SP.
                        b.pop(R4, BSP);
                        b.pop(R3, BSP);
                        // Push two items to RP.
                        b.push(R3, BRP);
                        b.push(R4, BRP);
                    }, State::Root),

                    // (LOOP)
                    build(opcode(0x56), |b| {
                        // Load the index and limit from RP.
                        b.pop(R3, BRP);
                        b.load(R4, BRP);
                        // Update the index.
                        b.const_binary(Add, R3, R3, 1);
                        b.push(R3, BRP);
                        b.binary(Sub, LoopFlag, R3, R4);
                    }, State::Loop),

                    // (LOOP)I
                    build(opcode(0x57), |b| {
                        // Load the index and limit from RP.
                        b.pop(R3, BRP);
                        b.load(R4, BRP);
                        // Update the index.
                        b.const_binary(Add, R3, R3, 1);
                        b.push(R3, BRP);
                        b.binary(Sub, LoopFlag, R3, R4);
                    }, State::Loopi),

                    // (+LOOP)
                    build(opcode(0x58), |b| {
                        // Pop the step from SP.
                        b.pop(LoopStep, BSP);
                        // Load the index and limit from RP.
                        b.pop(R3, BRP);
                        b.load(R4, BRP);
                        // Update the index.
                        b.binary(Add, R5, R3, LoopStep);
                        b.push(R5, BRP);
                        // Compute the differences between old and new indexes and limit.
                        b.binary(Sub, LoopOld, R3, R4);
                        b.binary(Sub, LoopNew, R5, R4);
                    }, State::Ploop),

                    // (+LOOP)I
                    build(opcode(0x59), |b| {
                        // Pop the step from SP.
                        b.pop(R2, BSP);
                        // Load the index and limit from RP.
                        b.pop(R3, BRP);
                        b.load(R4, BRP);
                        // Update the index.
                        b.binary(Add, R5, R3, R2);
                        b.push(R5, BRP);
                        // Compute the differences between old and new indexes and limit.
                        b.binary(Sub, LoopOld, R3, R4);
                        b.binary(Sub, LoopNew, R5, R4);
                    }, State::Ploopi),

                    // UNLOOP
                    build(opcode(0x5a), |b| {
                        // Discard two items from RP.
                        b.const_binary(Add, BRP, BRP, cell_bytes(2));
                    }, State::Root),

                    // J
                    build(opcode(0x5b), |b| {
                        // Push the third item of RP to SP.
                        b.const_binary(Add, R1, BRP, cell_bytes(2));
                        b.load(R4, R1);
                        b.push(R4, BSP);
                    }, State::Root),

                    // (LITERAL)
                    build(opcode(0x5c), |b| {
                        // Load R2 from cell pointed to by BEP, and add 4 to EP.
                        b.pop(R2, BEP); // FIXME: Add check that EP is now valid.
                        b.push(R2, BSP);
                    }, State::Root),

                    // (LITERAL)I
                    build(opcode(0x5d), |b| {
                        b.push(BA, BSP);
                    }, State::Next),

                    // THROW
                    build(opcode(0x5e), |b| {
                        b.move_(BBad, BEP);
                        b.load(BEP, BThrow); // FIXME: Add check that EP is valid.
                    }, State::Next),

                    // HALT
                    build(opcode(0x5f), |_| {}, State::Halt),
                ],
            ),
        }
    }

    fn initial_states(&self) -> Vec<Self::State> {
        vec![State::Root]
    }
}

//-----------------------------------------------------------------------------

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn ackermann_object() -> Vec<u32> {
        // Forth source:
        // : ACKERMANN   ( m n -- result )
        // OVER 0= IF                  \ m = 0
        //     NIP 1+                  \ n+1
        // ELSE
        //     DUP 0= IF               \ n = 0
        //         DROP 1- 1 RECURSE   \ A(m-1, 1)
        //     ELSE
        //         OVER 1- -ROT        \ m-1 m n
        //         1- RECURSE          \ m-1 A(m, n-1)
        //         RECURSE             \ A(m-1, A(m, n-1))
        //     THEN
        // THEN ;

        // Beetle assembler:
        // $00: OVER
        //      0=
        // $04: ?BRANCHI $10
        // $08: NIP
        //      1+
        // $0C: BRANCHI $30
        // $10: DUP
        //      0=
        // $14: ?BRANCHI $24
        // $18: DROP
        //      1-
        //      1
        // $1C: CALLI $0
        // $20: BRANCHI $30
        // $24: OVER
        //      1-
        //      -ROT
        //      1-
        // $28: CALLI $0
        // $2C: CALLI $0
        // $30: EXIT

        // Beetle object code:
        vec![
            0x00001504, 0x0000024F, 0x00002108, 0x0000084D,
            0x00001501, 0x0000034F, 0x001A2202, 0xFFFFF853,
            0x0000034D, 0x22062204, 0xFFFFF553, 0xFFFFF453,
            0x00000054,
        ]
    }

    use crate::{jit};

    /** The size of the Beetle memory, in cells. */
    const MEMORY_CELLS: usize = 1 << 20;
    /** The size of the Beetle data stack, in cells. */
    const DATA_CELLS: usize = 1 << 18;
    /** The size of the Beetle return stack, in cells. */
    const RETURN_CELLS: usize = 1 << 18;

    pub struct VM {
        /** The compiled code, registers, and other compiler state. */
        jit: jit::Jit<Machine>,
        /** The Beetle memory. */
        memory: Vec<u32>,
        /** The amount of unallocated memory, in cells. */
        free_cells: u32,
        /** The address of a HALT instruction. */
        halt_addr: u32,
    }

    impl VM {
        /**
         * Constructs a Beetle virtual machine with the specified parameters.
         *
         * The memory is `memory_cells` cells. The data stack occupies the last
         * `data_cells` cells of the memory, and the return stack occupies
         * the last `return_cells` cells before that. The cells before that
         * are free for the program's use.
         */
        pub fn new(
            memory_cells: usize,
            data_cells: usize,
            return_cells: usize,
        ) -> Self {
            assert!(memory_cells <= u32::MAX as usize);
            assert!(data_cells <= u32::MAX as usize);
            assert!(return_cells <= u32::MAX as usize);
            let mut vm = VM {
                jit: jit::Jit::new(Machine, jit::tests::CODE_SIZE),
                memory: (0..memory_cells).map(|_| 0).collect(),
                free_cells: memory_cells as u32,
                halt_addr: 0,
            };
            // Initialize the memory.
            *vm.jit.slot(BMemory) = vm.memory.as_mut_ptr() as u64;
            // Allocate the data stack.
            let s_base = vm.allocate(data_cells as u32);
            let sp = s_base + cell_bytes(data_cells as i64) as u32;
            vm.set(BS0, sp);
            vm.set(BSP, sp);
            // Allocate the return stack.
            let r_base = vm.allocate(return_cells as u32);
            let rp = r_base + cell_bytes(return_cells as i64) as u32;
            vm.set(BR0, rp);
            vm.set(BRP, rp);
            // Allocate a word to hold a HALT instruction.
            vm.halt_addr = vm.allocate(1);
            vm.store(vm.halt_addr, 0x5F);
            vm
        }

        /** Read a register. */
        pub fn get(&mut self, global: Global) -> u32 {
            *self.jit.slot(global) as u32
        }

        /** Write a register. */
        pub fn set(&mut self, global: Global, value: u32) {
            *self.jit.slot(global) = value as u64
        }

        /**
         * Allocate `cells` cells and return a Beetle pointer to them.
         * Allocation starts at the top of memory and is permanent.
         */
        pub fn allocate(&mut self, cells: u32) -> u32 {
            assert!(cells <= self.free_cells);
            self.free_cells -= cells;
            cell_bytes(self.free_cells as i64) as u32
        }

        /**
         * Load `object` at address zero, i.e. in the unallocated memory.
         */
        pub fn load_object(&mut self, object: &[u32]) {
            assert!(object.len() <= self.free_cells as usize);
            for (i, &cell) in object.iter().enumerate() {
                self.memory[i] = cell;
            }
        }

        /** Return the value of the word at address `addr`. */
        pub fn load(&mut self, addr: u32) -> u32 {
            assert_eq!(addr & 0x3, 0);
            self.memory[(addr >> 2) as usize]
        }

        /** Set the word at address `addr` to `value`. */
        pub fn store(&mut self, addr: u32, value: u32) {
            assert_eq!(addr & 0x3, 0);
            self.memory[(addr >> 2) as usize] = value;
        }

        /** Push `item` onto the data stack. */
        pub fn push(&mut self, item: u32) {
            let mut sp = self.get(BSP);
            sp -= cell_bytes(1) as u32;
            self.set(BSP, sp);
            self.store(sp, item);
        }

        /** Pop an item from the data stack. */
        pub fn pop(&mut self) -> u32 {
            let mut sp = self.get(BSP);
            let item = self.load(sp);
            sp += cell_bytes(1) as u32;
            self.set(BSP, sp);
            item
        }

        /** Push `item` onto the return stack. */
        pub fn rpush(&mut self, item: u32) {
            let mut rp = self.get(BRP);
            rp -= cell_bytes(1) as u32;
            self.set(BRP, rp);
            self.store(rp, item);
        }

        /** Pop an item from the return stack. */
        pub fn rpop(&mut self) -> u32 {
            let mut rp = self.get(BRP);
            let item = self.load(rp);
            rp += cell_bytes(1) as u32;
            self.set(BRP, rp);
            item
        }

        /** Run the code at address `ep`. */
        pub fn run(mut self, ep: u32) -> Self {
            assert!(Self::is_aligned(ep));
            self.set(BEP, ep);
            let (jit, state) = self.jit.execute(State::Root);
	    assert_eq!(state, State::Halt);
            self.jit = jit;
            self
        }

        /** Indicate whether an address is cell-aligned. */
        pub fn is_aligned(addr: u32) -> bool {
            addr & 0x3 == 0
        }
    }

    #[test]
    pub fn ackermann() {
        let mut vm = VM::new(MEMORY_CELLS, DATA_CELLS, RETURN_CELLS);
        vm.load_object(ackermann_object().as_ref());
        vm.push(3);
        vm.push(5);
        vm.rpush(vm.halt_addr);
        vm = vm.run(0);
        let result = vm.pop();
        assert_eq!(vm.get(BS0), vm.get(BSP));
        assert_eq!(vm.get(BR0), vm.get(BRP));
        assert_eq!(result, 253);
    }
}
