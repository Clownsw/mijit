use super::{code, target};

mod engine;
// TODO: Make private. Having these public reduces "unused" warnings.
pub use engine::{Engine, EntryId};

//mod machine;
//pub use machine::{Jit};

#[cfg(test)]
pub mod factorial;
