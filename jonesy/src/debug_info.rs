use goblin::Object;
use goblin::mach::symbols::N_OSO;
use goblin::mach::{Mach, MachO};
use ouroboros::self_referencing;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Self-referencing struct that owns the buffer and the parsed MachO that borrows from it
#[self_referencing]
pub struct DSymInfo {
    pub debug_buffer: Vec<u8>,
    #[borrows(debug_buffer)]
    #[covariant]
    pub debug_macho: Mach<'this>,
}

/// Information about an object file from the debug map
#[derive(Debug)]
pub struct ObjectFileInfo {
    /// Path to the object file
    pub path: PathBuf,
    /// Raw bytes of the object file
    pub buffer: Vec<u8>,
    /// Symbol address translations: object file address -> final binary address
    pub addr_map: HashMap<u64, u64>,
}

/// Debug map information parsed from the binary's symbol table
pub struct DebugMapInfo {
    /// Object files referenced by the debug map
    pub object_files: Vec<ObjectFileInfo>,
}

/// Debug info source - either embedded in binary or from a separate dSYM file/bundle
pub enum DebugInfo {
    /// Debug info is embedded in the binary
    Embedded,
    /// Debug info is in a separate dSYM bundle
    DSym(Box<DSymInfo>),
    /// Debug info from object files via debug map
    DebugMap(Box<DebugMapInfo>),
    /// No debug info available
    None,
}

impl DebugInfo {
    /// Returns a reference to the debug MachO if this is a DSym variant
    pub fn debug_macho(&self) -> Option<&Mach<'_>> {
        match self {
            DebugInfo::DSym(info) => Some(info.borrow_debug_macho()),
            _ => None,
        }
    }

    /// Returns a reference to the debug buffer if this is a DSym variant
    pub fn debug_buffer(&self) -> Option<&[u8]> {
        match self {
            DebugInfo::DSym(info) => Some(info.borrow_debug_buffer()),
            _ => None,
        }
    }
}

pub fn find_dsym(binary_path: &Path) -> Option<PathBuf> {
    // dSYM is typically at: /path/to/binary.dSYM/Contents/Resources/DWARF/binary
    let dsym_bundle = binary_path.with_extension("dSYM");

    if dsym_bundle.exists() {
        let binary_name = binary_path.file_name()?;
        let dwarf_path = dsym_bundle
            .join("Contents")
            .join("Resources")
            .join("DWARF")
            .join(binary_name);

        if dwarf_path.exists() {
            return Some(dwarf_path);
        }
    }
    None
}

/// Return true if `macho` has a `__DWARF` segment or a section named `__debug_*` in any segment
fn has_dwarf_sections(macho: &MachO) -> bool {
    for segment in macho.segments.iter() {
        if let Ok(name) = segment.name()
            && name == "__DWARF"
        {
            return true;
        }

        // Also check for debug sections in any segment
        if let Ok(sections) = segment.sections() {
            for (section, _) in sections {
                if let Ok(name) = section.name()
                    && name.starts_with("__debug_")
                {
                    return true;
                }
            }
        }
    }

    false
}

/// Extract object file paths from the debug map (OSO stab entries)
fn get_oso_paths(macho: &MachO) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(symbols) = &macho.symbols {
        for (name, nlist) in symbols.iter().flatten() {
            // N_OSO (0x66) indicates an object file reference
            if nlist.n_type == N_OSO && !name.is_empty() {
                paths.push(PathBuf::from(name));
            }
        }
    }

    // Deduplicate paths
    paths.sort();
    paths.dedup();
    paths
}

/// Build address translation map from object file symbols to final binary addresses
fn build_addr_translation_map(binary_macho: &MachO, obj_macho: &MachO) -> HashMap<u64, u64> {
    let mut addr_map = HashMap::new();

    // Get symbols from both binary and object file
    let Some(binary_symbols) = &binary_macho.symbols else {
        return addr_map;
    };
    let Some(obj_symbols) = &obj_macho.symbols else {
        return addr_map;
    };

    // Build a map of symbol name -> address in binary
    let mut binary_sym_addrs: HashMap<String, u64> = HashMap::new();
    for (name, nlist) in binary_symbols.iter().flatten() {
        if nlist.n_value > 0 && !name.is_empty() {
            binary_sym_addrs.insert(name.to_string(), nlist.n_value);
        }
    }

    // For each symbol in the object file, find its final address in the binary
    for (name, nlist) in obj_symbols.iter().flatten() {
        if nlist.n_value > 0
            && !name.is_empty()
            && let Some(&binary_addr) = binary_sym_addrs.get(name)
        {
            addr_map.insert(nlist.n_value, binary_addr);
        }
    }

    addr_map
}

/// Check if a dSYM bundle is stale (binary is newer than the dSYM)
fn is_dsym_stale(binary_path: &Path, dsym_path: &Path) -> bool {
    let binary_modified = match fs::metadata(binary_path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return false, // Can't check, assume not stale
    };

    let dsym_modified = match fs::metadata(dsym_path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return true, // Can't read dSYM metadata, regenerate
    };

    binary_modified > dsym_modified
}

// 1) No embedded debug info, no dSYM
// 2) No embedded debug info, dSYM
// 3) Embedded debug info, no dSYM
// 4) Embedded debug info, dSYM
pub fn load_debug_info(macho: &MachO, binary_path: &Path, quiet: bool) -> DebugInfo {
    // Look for dSYM symbol directory
    // Try both with and without extension since dsymutil behavior varies
    let file_name = binary_path.file_name().unwrap().to_str().unwrap();
    let file_stem = binary_path.file_stem().unwrap().to_str().unwrap();

    // Try .dSYM bundle with full filename first
    let dsym_base = binary_path.parent().unwrap_or(Path::new("."));
    let dsym_paths = [
        // Pattern: binary.dSYM/Contents/Resources/DWARF/binary
        dsym_base
            .join(format!("{}.dSYM", file_stem))
            .join("Contents/Resources/DWARF")
            .join(file_name),
        // Pattern: binary.dSYM/Contents/Resources/DWARF/binary (without extension)
        dsym_base
            .join(format!("{}.dSYM", file_stem))
            .join("Contents/Resources/DWARF")
            .join(file_stem),
        // Pattern: binary.ext.dSYM/Contents/Resources/DWARF/binary.ext
        binary_path
            .with_extension("dSYM")
            .join("Contents/Resources/DWARF")
            .join(file_name),
    ];

    for dsym_path in &dsym_paths {
        if dsym_path.exists() {
            // Check if dSYM is stale (binary is newer than dSYM)
            let dsym_stale = is_dsym_stale(binary_path, dsym_path);
            if dsym_stale {
                if !quiet {
                    println!("  dSYM is stale, will regenerate");
                }
            } else {
                if !quiet {
                    println!("  Using .dSYM bundle for debug info");
                }
                let debug_buffer = fs::read(dsym_path).unwrap();
                let dsym_info = DSymInfoBuilder {
                    debug_buffer,
                    debug_macho_builder: |buf: &Vec<u8>| Mach::parse(buf).unwrap(),
                }
                .build();
                return DebugInfo::DSym(Box::new(dsym_info));
            }
        }
    }

    if has_dwarf_sections(macho) {
        if !quiet {
            println!("  Using embedded DWARF debugging info");
        }
        return DebugInfo::Embedded;
    }

    // Try to auto-generate dSYM using dsymutil
    if let Some(dsym_info) = auto_generate_dsym(binary_path, quiet) {
        return DebugInfo::DSym(Box::new(dsym_info));
    }

    // Fall back to debug map (reading DWARF from object files)
    if let Some(debug_map) = load_debug_map(macho, quiet) {
        return DebugInfo::DebugMap(Box::new(debug_map));
    }

    if !quiet {
        println!("  No debug info found (no dSYM, embedded DWARF, or debug map)");
        println!(
            "Tip: Install dsymutil or run 'dsymutil {}' to generate debug symbols",
            binary_path.display()
        );
    }
    DebugInfo::None
}

/// Auto-generate dSYM by running dsymutil
fn auto_generate_dsym(binary_path: &Path, quiet: bool) -> Option<DSymInfo> {
    use std::process::Command;

    let dsym_path = binary_path.with_extension("dSYM");

    // Check if dsymutil is available
    let status = Command::new("dsymutil")
        .arg(binary_path)
        .arg("-o")
        .arg(&dsym_path)
        .status()
        .ok()?;

    if !status.success() {
        return None;
    }

    // Find the DWARF file inside the dSYM bundle
    let file_name = binary_path.file_name()?.to_str()?;
    let file_stem = binary_path.file_stem()?.to_str()?;

    let dwarf_paths = [
        dsym_path.join("Contents/Resources/DWARF").join(file_name),
        dsym_path.join("Contents/Resources/DWARF").join(file_stem),
    ];

    for dwarf_path in &dwarf_paths {
        if dwarf_path.exists() {
            if !quiet {
                println!("  Generated .dSYM bundle for debug info");
            }
            let debug_buffer = fs::read(dwarf_path).ok()?;
            let dsym_info = DSymInfoBuilder {
                debug_buffer,
                debug_macho_builder: |buf: &Vec<u8>| Mach::parse(buf).unwrap(),
            }
            .build();
            return Some(dsym_info);
        }
    }

    None
}

/// Load debug map from the binary's symbol table
/// This reads OSO entries and loads DWARF from the referenced object files
fn load_debug_map(macho: &MachO, quiet: bool) -> Option<DebugMapInfo> {
    let oso_paths = get_oso_paths(macho);

    if oso_paths.is_empty() {
        return None;
    }

    let mut object_files = Vec::new();
    let mut loaded_count = 0;

    for path in oso_paths {
        // Skip if object file doesn't exist
        if !path.exists() {
            continue;
        }

        // Read the object file
        let Ok(buffer) = fs::read(&path) else {
            continue;
        };

        // Parse as MachO and check for DWARF
        let Ok(Object::Mach(Mach::Binary(obj_macho))) = Object::parse(&buffer) else {
            continue;
        };

        // Only include if it has debug info
        if !has_dwarf_sections(&obj_macho) {
            continue;
        }

        // Build address translation map
        let addr_map = build_addr_translation_map(macho, &obj_macho);

        object_files.push(ObjectFileInfo {
            path,
            buffer,
            addr_map,
        });
        loaded_count += 1;
    }

    if object_files.is_empty() {
        return None;
    }

    if !quiet {
        println!(
            "Using debug map: loaded {} object files with DWARF",
            loaded_count
        );
    }

    Some(DebugMapInfo { object_files })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // Tests for dSYM detection and staleness
    // ========================================================================

    #[test]
    fn test_is_dsym_stale_binary_newer() {
        use std::fs;
        use std::thread;
        use std::time::Duration;

        let temp_dir = std::env::temp_dir().join("jonesy_test_dsym_stale");
        let _ = fs::create_dir_all(&temp_dir);

        let binary_path = temp_dir.join("test_binary");
        let dsym_path = temp_dir.join("test_binary.dSYM");

        // Create dSYM first (older)
        fs::write(&dsym_path, "fake dsym").unwrap();

        // Wait a bit to ensure different timestamps
        thread::sleep(Duration::from_millis(50));

        // Create binary second (newer)
        fs::write(&binary_path, "fake binary").unwrap();

        // Binary is newer than dSYM, so dSYM is stale
        assert!(
            is_dsym_stale(&binary_path, &dsym_path),
            "dSYM should be stale when binary is newer"
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_is_dsym_stale_dsym_newer() {
        use std::fs;
        use std::thread;
        use std::time::Duration;

        let temp_dir = std::env::temp_dir().join("jonesy_test_dsym_fresh");
        let _ = fs::create_dir_all(&temp_dir);

        let binary_path = temp_dir.join("test_binary");
        let dsym_path = temp_dir.join("test_binary.dSYM");

        // Create binary first (older)
        fs::write(&binary_path, "fake binary").unwrap();

        // Wait a bit to ensure different timestamps
        thread::sleep(Duration::from_millis(50));

        // Create dSYM second (newer)
        fs::write(&dsym_path, "fake dsym").unwrap();

        // dSYM is newer than binary, so dSYM is not stale
        assert!(
            !is_dsym_stale(&binary_path, &dsym_path),
            "dSYM should not be stale when dSYM is newer"
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_is_dsym_stale_binary_not_found() {
        use std::fs;

        let temp_dir = std::env::temp_dir().join("jonesy_test_dsym_no_binary");
        let _ = fs::create_dir_all(&temp_dir);

        let binary_path = temp_dir.join("nonexistent_binary");
        let dsym_path = temp_dir.join("test.dSYM");

        // Create dSYM but not binary
        fs::write(&dsym_path, "fake dsym").unwrap();

        // When binary doesn't exist, function returns false (can't check)
        assert!(
            !is_dsym_stale(&binary_path, &dsym_path),
            "Should return false when binary doesn't exist"
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    // ========================================================================
    // Tests for find_dsym
    // ========================================================================

    #[test]
    fn test_find_dsym_with_existing_dsym() {
        let temp_dir = std::env::temp_dir().join("jonesy_test_find_dsym_exists");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let binary_path = temp_dir.join("my_binary");
        fs::write(&binary_path, "fake binary").unwrap();

        // Create the dSYM bundle structure
        let dwarf_dir = temp_dir
            .join("my_binary.dSYM")
            .join("Contents")
            .join("Resources")
            .join("DWARF");
        fs::create_dir_all(&dwarf_dir).unwrap();
        let dwarf_file = dwarf_dir.join("my_binary");
        fs::write(&dwarf_file, "fake dwarf").unwrap();

        let result = find_dsym(&binary_path);
        assert_eq!(result, Some(dwarf_file));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_find_dsym_no_dsym_bundle() {
        let temp_dir = std::env::temp_dir().join("jonesy_test_find_dsym_none");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let binary_path = temp_dir.join("my_binary");
        fs::write(&binary_path, "fake binary").unwrap();

        assert_eq!(find_dsym(&binary_path), None);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_find_dsym_bundle_exists_but_no_dwarf_file() {
        let temp_dir = std::env::temp_dir().join("jonesy_test_find_dsym_empty");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let binary_path = temp_dir.join("my_binary");
        fs::write(&binary_path, "fake binary").unwrap();

        // Create bundle dir but no DWARF file inside
        let dwarf_dir = temp_dir
            .join("my_binary.dSYM")
            .join("Contents")
            .join("Resources")
            .join("DWARF");
        fs::create_dir_all(&dwarf_dir).unwrap();

        assert_eq!(find_dsym(&binary_path), None);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    // ========================================================================
    // Tests for DebugInfo enum
    // ========================================================================

    #[test]
    fn test_debug_info_embedded_has_no_debug_macho() {
        let di = DebugInfo::Embedded;
        assert!(di.debug_macho().is_none());
        assert!(di.debug_buffer().is_none());
    }

    #[test]
    fn test_debug_info_none_has_no_debug_macho() {
        let di = DebugInfo::None;
        assert!(di.debug_macho().is_none());
        assert!(di.debug_buffer().is_none());
    }

    #[test]
    fn test_is_dsym_stale_dsym_not_found() {
        use std::fs;

        let temp_dir = std::env::temp_dir().join("jonesy_test_dsym_no_dsym");
        let _ = fs::create_dir_all(&temp_dir);

        let binary_path = temp_dir.join("test_binary");
        let dsym_path = temp_dir.join("nonexistent.dSYM");

        // Create binary but not dSYM
        fs::write(&binary_path, "fake binary").unwrap();

        // When dSYM doesn't exist, function returns true (needs regeneration)
        assert!(
            is_dsym_stale(&binary_path, &dsym_path),
            "Should return true when dSYM doesn't exist"
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
