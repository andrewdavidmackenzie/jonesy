//! Architecture-specific code for binary analysis.
//!
//! This module provides architecture-specific functionality for:
//! - Instruction disassembly and decoding
//! - Relocation type constants (MachO and ELF)
//! - PLT (Procedure Linkage Table) resolution
//!
//! # Supported Architectures
//!
//! Currently supported:
//! - **aarch64** (ARM64) - macOS and Linux
//!
//! # Adding New Architecture Support
//!
//! To add support for a new architecture (e.g., x86_64):
//!
//! 1. Create `arch/<arch>.rs` module with required functions
//! 2. Add `#[cfg(target_arch = "<arch>")]` block below
//! 3. Update compile_error check to include new architecture
//! 4. Implement architecture-specific constants and functions:
//!    - `INSN_SIZE` - instruction size in bytes
//!    - `decode_branch_target()` - decode branch/call instructions
//!    - `scan_branch_instructions()` - find call sites in code
//!    - Relocation constants for MachO and ELF
//!    - PLT resolution (if applicable for ELF shared libraries)
//!
//! # Architecture-Specific Assumptions
//!
//! Each architecture module documents its specific assumptions about:
//! - Instruction encoding (fixed-size vs variable-size ISA)
//! - Branch/call instruction formats
//! - Relocation types for linking
//! - PLT entry layout and size (ELF shared libraries)

// Compile-time error for unsupported architectures
#[cfg(not(target_arch = "aarch64"))]
compile_error!("jonesy only supports aarch64 architecture");

// Architecture-specific modules
#[cfg(target_arch = "aarch64")]
pub mod aarch64;

// Re-export the current architecture's functionality
#[cfg(target_arch = "aarch64")]
pub use self::aarch64::*;
