# Jonesy Performance Benchmarks

This document describes how to run performance benchmarks for jonesy across different architectures.

## Platform Information

Record your platform details:
- **OS**: (e.g., Linux, macOS)
- **Architecture**: (e.g., x86_64, aarch64)
- **CPU**: (from `lscpu` on Linux or `sysctl -n machdep.cpu.brand_string` on macOS)
- **Rust Version**: (from `rustc --version`)
- **Jonesy Version**: (from `cargo run -- --version`)

## Prerequisites

1. Build jonesy in release mode:
   ```bash
   cargo build --release -p jonesy
   ```

2. Build all test projects in debug mode (jonesy analyzes debug binaries):
   ```bash
   # In jonesy repo
   make build-examples
   
   # External projects (if available)
   cd ../meshcore-rs && cargo build
   cd ../meshchat && cargo build
   cd ../flow && cargo build
   ```

## Running Benchmarks

Use the provided benchmark script:

```bash
cd /path/to/jonesy
./benchmark.sh
```

This will:
1. Run jonesy on each test binary 3 times
2. Report minimum, median, and maximum times
3. Count panic detections for verification
4. Save results to `benchmark_results_<platform>_<date>.txt`

## Manual Benchmark Instructions

If you prefer to run manually:

```bash
# Create results file
PLATFORM="$(uname -s)-$(uname -m)"
DATE="$(date +%Y%m%d)"
RESULTS="benchmark_results_${PLATFORM}_${DATE}.txt"

# Record platform info
{
  echo "Platform: $(uname -s) $(uname -m)"
  echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  rustc --version
  echo "Jonesy: $(cargo run --release -p jonesy -- --version 2>/dev/null || echo 'unknown')"
  echo ""
} > "$RESULTS"

# Benchmark function
benchmark_binary() {
  local name="$1"
  local binary="$2"
  
  echo "=== $name ===" | tee -a "$RESULTS"
  
  # Run 3 times and extract timing
  for i in 1 2 3; do
    /usr/bin/time -f "%E elapsed" \
      cargo run --release -p jonesy -- --bin "$binary" 2>&1 | \
      grep "elapsed" | tee -a "$RESULTS"
  done
  
  # Count detections (for verification)
  COUNT=$(cargo run --release -p jonesy -- --bin "$binary" 2>/dev/null | \
    grep -E "Panic points: [0-9]+" | grep -oE "[0-9]+" || echo "0")
  echo "Panic points: $COUNT" | tee -a "$RESULTS"
  echo "" | tee -a "$RESULTS"
}

# Run benchmarks on each binary
benchmark_binary "simple_panic" "target/debug/simple_panic"
benchmark_binary "panic" "target/debug/panic"
benchmark_binary "perfect" "target/debug/perfect"
# ... add more as needed
```

## Test Binaries

### Jonesy Examples
- `simple_panic`: Basic panic scenarios (~20 panic points)
- `panic`: Comprehensive panic test suite (~100+ panic points)
- `perfect`: Example with no panics
- `rlib`: Static library analysis
- `dylib`: Dynamic library analysis
- `workspace_test`: Multi-package workspace

### External Projects
- `meshcore-rs`: Router mesh core library
- `meshchat`: Chat application
- `flow`: Dataflow framework

## Expected Timing Ranges

These are rough guidelines (will vary by platform):

| Binary | Size | Panic Points | Expected Time |
|--------|------|--------------|---------------|
| simple_panic | ~1MB | ~20 | 0.5-2s |
| panic | ~2MB | ~100 | 1-5s |
| perfect | ~1MB | 0 | 0.5-2s |
| meshcore-rs | ~5MB | varies | 2-10s |
| meshchat | ~10MB | varies | 5-20s |
| flow | ~15MB | varies | 10-30s |

## Comparing Results

After running on multiple platforms:

1. Compare median times across architectures
2. Calculate relative performance: `time_x86_64 / time_aarch64`
3. Identify bottlenecks (e.g., disassembly, DWARF parsing, GOT resolution)
4. Look for architecture-specific optimizations

## Notes

- Run benchmarks on an idle system for consistency
- The first run may be slower due to cold caches
- Use release builds of jonesy for meaningful timings
- Test binaries should be debug builds (with DWARF info)
