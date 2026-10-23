//! Pure logic of the update archive read API (`app/trove/updates/read.py`,
//! `compare.py`, and the `.pkfx` parser the VFX browser shares with the Mods Hub).

pub mod compare;
pub mod difflib;
pub mod pkfx;
pub mod tree;

#[cfg(test)]
mod tests;
