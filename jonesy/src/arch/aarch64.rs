//! ARM64/aarch64 architecture-specific code.
//!
//! This module contains all ARM64-specific logic for binary analysis:
//! - Instruction encoding and decoding
//! - Branch instruction disassembly
//! - Relocation type constants
//! - PLT (Procedure Linkage Table) resolution for ELF shared libraries
//!
//! # Architecture Characteristics
//!
//! ARM64 has a **fixed-size instruction set** where all instructions are exactly 4 bytes.
//! This makes instruction scanning and disassembly straightforward compared to variable-length
//! ISAs like x86_64.
//!
//! ## Branch Instructions
//!
//! ARM64 uses two primary branch instructions for function calls:
//! - **BL (Branch with Link)**: Used for direct function calls
//! - **B (Branch)**: Used for tail calls and jumps
//!
//! Both use PC-relative addressing with a 26-bit signed offset (±128MB range).

/// ARM64 instruction size in bytes (fixed-size ISA).
pub const INSN_SIZE: usize = 4;

// Branch instruction encoding constants will be moved here in Phase 2

/// PLT (Procedure Linkage Table) resolution for ELF shared libraries.
///
/// This submodule handles mapping PLT stub addresses to actual function addresses,
/// which is necessary for analyzing calls in ELF shared objects (.so files).
pub mod plt {
    // PLT constants and functions will be moved here in Phase 4
}
