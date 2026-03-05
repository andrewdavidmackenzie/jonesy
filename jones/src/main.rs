use crate::args::parse_args;
use crate::sym::{
    find_callers, find_callers_with_debug_info, find_symbol_address, find_symbol_containing, load_debug_info,
    read_symbols, DebugInfo, SymbolTable,
};
use goblin::mach::Mach::{Binary, Fat};
use std::error::Error;
use std::fs;

mod args;
#[cfg(target_os = "macos")]
mod sym;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();

    let binaries = parse_args(&args).unwrap_or_else(|e| {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    });

    for binary_path in binaries {
        println!("Processing {}", binary_path.display());

        let binary_buffer = fs::read(&binary_path)?;
        let symbols = read_symbols(&binary_buffer)?;

        match symbols {
            SymbolTable::MachO(Binary(macho)) => {
                // Find symbols with panic in them (regex pattern)
                let target_symbol = "rust_panic$";
                if let Ok(Some((panic_symbol, demangled))) =
                    find_symbol_containing(&macho, target_symbol)
                {
                    // Find the target symbol's address
                    match find_symbol_address(&macho, &panic_symbol) {
                        Some((_sym_name, target_addr)) => {
                            println!("Symbol {demangled}");
                            let debug_info = load_debug_info(&macho, &binary_path);
                            match &debug_info {
                                DebugInfo::Embedded => {
                                    call_tree(&macho, &binary_buffer, &debug_info, target_addr, 1);
                                }
                                DebugInfo::DSym(_) => {
                                    call_tree(&macho, &binary_buffer, &debug_info, target_addr, 1);
                                }
                                DebugInfo::None => {
                                    println!("No debug info found, looking for callers by address");
                                    call_tree(&macho, &binary_buffer, &debug_info, target_addr, 1);
                                }
                            }
                        }
                        None => println!("Couldn't find '{}' address", panic_symbol),
                    }
                } else {
                    println!("No references to '{}' found", target_symbol);
                }
            }
            SymbolTable::MachO(Fat(multi_arch)) => {
                println!("FAT: {:?} architectures", multi_arch.arches().unwrap());
            }
        }

        println!();
    }

    Ok(())
}

// TODO Maybe have a list of internal rust symbols that is used to filter out call tree
// paths that we are not interested in, as this finds A LOT of paths, some that don't even
// make it up to main (like signal handling)
// std::rt::lang_start
// std::sys::pal::unix::stack_overflow::imp::signal_handler
// Construct a Graph or DAG that can be filtered, inverted and printed out or drawn (dot?) later?
fn call_tree(
    binary_macho: &goblin::mach::MachO,
    binary_buffer: &[u8],
    debug_source: &DebugInfo,
    target_addr: u64,
    depth: usize,
) {
    let callers = match debug_source {
        DebugInfo::Embedded => {
            // Binary and debug are the same
            find_callers_with_debug_info(
                binary_macho,
                binary_buffer,
                binary_macho,
                binary_buffer,
                target_addr,
            )
            .unwrap()
        }
        DebugInfo::DSym(dsym_info) => {
            // Binary for code, dSYM for debug info
            // Use ouroboros-generated accessors to get references
            dsym_info.with_debug_macho(|debug_macho| {
                if let goblin::mach::Mach::Binary(macho) = debug_macho {
                    find_callers_with_debug_info(
                        binary_macho,
                        binary_buffer,
                        macho,
                        dsym_info.borrow_debug_buffer(),
                        target_addr,
                    )
                    .unwrap()
                } else {
                    find_callers(binary_macho, binary_buffer, target_addr).unwrap()
                }
            })
        }
        DebugInfo::None => {
            // No debug info, use symbol table only
            find_callers(binary_macho, binary_buffer, target_addr).unwrap()
        }
    };

    let indent = "    ".repeat(depth);
    for caller_info in callers {
        match (&caller_info.file, &caller_info.line) {
            (Some(filename), None) => println!(
                "{}Called from: '{}' (source: {}",
                indent, caller_info.caller.name, filename
            ),
            (Some(filename), Some(line)) => println!(
                "{}Called from: '{}' (source: {}:{})",
                indent, caller_info.caller.name, filename, line
            ),
            _ => println!("{}Called from: '{}'", indent, caller_info.caller.name),
        }
        // Recurse using the caller's function start address, not the call site
        call_tree(
            binary_macho,
            binary_buffer,
            debug_source,
            caller_info.caller.start_address,
            depth + 1,
        );
    }
}
