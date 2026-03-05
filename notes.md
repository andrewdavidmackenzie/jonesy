# Mach-O

Contains:

- header
- load commands
- segments, that contain sections. Convention is to name segments in uppercase prefixed by two underscores (e.g., __
  TEXT)

## Header

Structure identifying file as a Mach-O executable. Contains general information about file.

struct mach_header {
unsigned long magic; /* Mach magic number identifier */
cpu_type_t cputype; /* cpu specifier */
cpu_subtype_t cpusubtype; /* machine specifier */
unsigned long filetype; /* type of file */
unsigned long ncmds; /* number of load commands */
unsigned long sizeofcmds; /* size of all load commands */
unsigned long flags; /* flags */
};

## Sections

Sections contain code or data of different types. Convention is to name sections in lowercase prefixed by two
underscores (e.g., __text)

## Text Segment and main Sections

- __PAGEZERO: One full VM page (4096 bytes or 0–0x1000) located at 0 with no protection rights assigned, which causes
  any accesses to c NULL to crash. With no data contained, it occupies no space in the file — file size is 0.
  -__TEXT Segment: Read-only area containing executable code and constant data. Compiler tools create every executable
  with at least one read-only __TEXT segment. Since read-only, can map directly into memory just once — all processes
  can share safely (mostly useful in frameworks and shared libraries, but also running same executable multiple times
  simultaneously). Major sections:
- __TEXT,__text: executable machine code
- __TEXT,__stubs/__stubs/helper: helpers involved in call to dynamically linked functions
- __TEXT,__cstring: constant c style (null terminated) strings. Duplicate strings removed by static linker when building
  final file.
- __TEXT,__picsymbol_stub: Position-independent symbol stubs, allow dynamic linker to load region of code at non-fixed
  virtual memory addresses.
- __TEST,__symbol_stub: Indirect symbol stubs.
- __TEXT,__const: initialized constant variables. All nonrelocatable const variables placed here. Uninitialized constant
  variables placed in a zero filled section.
- __TEXT,__literal4: 4-byte literal values, single precision floating point constants.
- __TEXT,__literal8: 8-byte literal values, double precision floating point constants. Sometimes more efficient to use
  immediate load instructions.
- __DATA Segment: Contains writable data, static linker sets the virtual memory permissions to allow both reading and
  writing. Because writable, segment is logically copied for each process linking with the library and marked as
  copy-on-write — when process writes to one of these pages, it receives its own private copy of the page.
- __DATA,__data: Initialized mutable varaibles
- __DATA,__la_symbol_ptr: Lazy symbol pointers — indirect references to data items imported from a different file.
- __DATA,__dyld: Placeholder section used by the dynamic linker
- __DATA,__const: Initialized relocatable constant variables.
- __DATA,__mod_init_func: Module initialization functions (e.g., C++ static constructors)
- __DATA,__mod_term_func: Module termination functions
- __DATA,__bss: uninitialized static variables (e.g., static int i;)
- __DATA,__common: Uninitialized imported symbol definitions (e.g., int i;, located in the global scope
- __OBJC Segment: Contains data used by the objective-c language runtime support library.
- __IMPORT Segment: contains symbol stubs and non-lazy pointers to symbols not defined in the executable. Generated only
  for executable targeted for the IA-32 architecture.
- __IMPORT,__jump_table: Stubs for calls to function in dynamic library
- __IMPORT,__pointers: Non-lazy symbol pointers — direct references to function imported from a different file.
- __LINKEDIT Segment: contains raw data used by the dynamic linker: symbol/string/relocation table entries.

## References

- [Understanding the Mach-O File Format](https://medium.com/@travmath/understanding-the-mach-o-file-format-66cf0354e3f4)
- [Overview of the Mach-O Executable Format](https://developer.apple.com/library/archive/documentation/Performance/Conceptual/CodeFootprint/Articles/MachOOverview.html)
- [Improving Locality of Reference](https://developer.apple.com/library/archive/documentation/Performance/Conceptual/CodeFootprint/Articles/ImprovingLocality.html)

# DWARF

https://dwarfstd.org/doc/Debugging%20using%20DWARF-2012.pdf

# Rust Panic Breakpoints

How IDEs and debuggers identify and set breakpoints on Rust panic symbols.

## Primary Symbols

| Symbol | Description |
|--------|-------------|
| `rust_panic` | Main panic entry point (now mangled in recent Rust) |
| `rust_begin_unwind` | Called early in panic unwinding |
| `std::panicking::rust_panic_with_hook` | Internal panic handler |
| `__rustc::rust_panic` | Mangled form of rust_panic |

## Implementation by Debugger

### GDB
```gdb
set breakpoint pending on
break rust_panic
# or
break rust_begin_unwind
```

### LLDB
```lldb
breakpoint set -n rust_panic
# or with regex for mangled symbols:
breakpoint set -r "rust_panic$"
```

### CodeLLDB (VS Code)
Sets a function breakpoint on `rust_panic`. Recent Rust versions mangle this symbol, so regex breakpoints are needed:
```
br s -r "rust_panic$"
```
Resolves to symbols like `__rustc::rust_panic`.

### RustRover / JetBrains IDEs
- "Break on panic" enabled by default
- Uses underlying LLDB debugger
- Setting: Preferences -> Build, Execution, Deployment -> Debugger -> Break on panic

## Symbol Mangling Issue

Recent Rust changes (rust-lang/rust#140821) mangle `rust_panic`, so it appears as:
- `rust_panic.llvm.5A8AA348` (varies by build)
- `__rustc::rust_panic`

Discover actual symbol name with:
```bash
# LLDB
image lookup -r -n rust_panic

# nm
nm target/debug/your_binary | grep rust_panic
```

## References

- https://github.com/rust-lang/rust/issues/21102
- https://github.com/vadimcn/codelldb/issues/1336
- https://github.com/rust-lang/rust/issues/49013
- https://github.com/intellij-rust/intellij-rust/issues/4763