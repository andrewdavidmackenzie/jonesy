#!/bin/bash
set -e

# Jonesy Performance Benchmark Script
# Runs jonesy on multiple test binaries and records timing

PLATFORM="$(uname -s)-$(uname -m)"
DATE="$(date +%Y%m%d_%H%M%S)"
RESULTS="benchmark_results_${PLATFORM}_${DATE}.txt"

echo "Starting benchmarks on $PLATFORM"
echo "Results will be saved to: $RESULTS"
echo ""

# Record platform information
{
  echo "========================================"
  echo "Jonesy Performance Benchmark"
  echo "========================================"
  echo ""
  echo "Platform: $(uname -s) $(uname -m)"
  echo "Date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo ""

  # CPU info
  if [ "$(uname -s)" = "Darwin" ]; then
    echo "CPU: $(sysctl -n machdep.cpu.brand_string)"
    echo "Cores: $(sysctl -n hw.ncpu)"
  else
    echo "CPU: $(lscpu | grep "Model name" | cut -d: -f2 | xargs)"
    echo "Cores: $(nproc)"
  fi
  echo ""

  rustc --version
  cargo --version
  echo ""

  # Build jonesy in release mode
  echo "Building jonesy (release)..."
  cargo build --release -p jonesy --quiet

  JONESY_VERSION=$(./target/release/jonesy --version 2>/dev/null || echo "unknown")
  echo "Jonesy: $JONESY_VERSION"
  echo ""
  echo "========================================"
  echo ""
} > "$RESULTS"

# Function to benchmark a single binary
benchmark_binary() {
  local name="$1"
  local binary="$2"

  if [ ! -f "$binary" ]; then
    echo "⊘ Skipping $name (binary not found: $binary)" | tee -a "$RESULTS"
    echo "" >> "$RESULTS"
    return
  fi

  echo "Benchmarking: $name" | tee -a "$RESULTS"

  # Get binary size
  local size=$(ls -lh "$binary" | awk '{print $5}')
  echo "Binary size: $size" >> "$RESULTS"

  # Convert to absolute path
  local abs_binary="$(cd "$(dirname "$binary")" && pwd)/$(basename "$binary")"

  # Run 3 times and collect timings
  local times=()
  for i in 1 2 3; do
    echo -n "  Run $i/3... "

    # Use high-precision timing (run from jonesy/ directory to avoid workspace issues)
    if command -v python3 >/dev/null 2>&1; then
      # Python3 for high precision timing
      elapsed=$(python3 -c "
import subprocess, time, os
os.chdir('jonesy')
start = time.time()
subprocess.run(['../target/release/jonesy', '--bin', '$abs_binary'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
elapsed = time.time() - start
print(f'{elapsed:.3f}')
")
      times+=("$elapsed")
      echo "${elapsed}s"
    else
      # Linux: use date with nanoseconds
      START=$(date +%s.%N)
      (cd jonesy && ../target/release/jonesy --bin "$abs_binary" >/dev/null 2>&1)
      END=$(date +%s.%N)
      elapsed=$(echo "$END - $START" | bc -l | xargs printf "%.3f")
      times+=("$elapsed")
      echo "${elapsed}s"
    fi
  done

  # Sort times and get min/median/max
  IFS=$'\n' sorted=($(sort -n <<<"${times[*]}"))
  min="${sorted[0]}"
  median="${sorted[1]}"
  max="${sorted[2]}"

  echo "  Times: min=${min}s, median=${median}s, max=${max}s" | tee -a "$RESULTS"

  # Count panic points (for verification)
  COUNT=$((cd jonesy && ../target/release/jonesy --bin "$abs_binary" 2>/dev/null) | \
    sed -nE 's/.*Panic points: ([0-9]+).*/\1/p' | head -n1)
  COUNT="${COUNT:-0}"
  echo "  Panic points detected: $COUNT" | tee -a "$RESULTS"

  echo "" >> "$RESULTS"
  echo ""
}

# Ensure examples are built
echo "Building examples..."
make build-examples >/dev/null 2>&1 || {
  echo "Warning: 'make build-examples' failed, some examples may be missing"
}

echo ""
echo "Running benchmarks..."
echo ""

# Benchmark jonesy examples
benchmark_binary "simple_panic" "target/debug/simple_panic"
benchmark_binary "panic" "target/debug/panic"
benchmark_binary "perfect" "target/debug/perfect"
benchmark_binary "rlib_example" "target/debug/librlib_example.rlib"
if [ "$(uname)" = "Darwin" ]; then
  benchmark_binary "dylib_example" "target/debug/libdylib_example.dylib"
else
  benchmark_binary "dylib_example" "target/debug/libdylib_example.so"
fi
benchmark_binary "staticlib_example" "target/debug/libstaticlib_example.a"
benchmark_binary "workspace_test_main" "target/debug/workspace_test_main"

# Benchmark external projects (if available)
if [ -d "../meshcore-rs" ]; then
  echo "Building meshcore-rs..."
  (cd ../meshcore-rs && cargo build --quiet 2>/dev/null) || true

  # Find meshcore-rs binaries
  for bin in ../meshcore-rs/target/debug/meshcore-*; do
    if [ -f "$bin" ] && [ -x "$bin" ] && [[ ! "$bin" =~ \.d$ ]]; then
      benchmark_binary "meshcore-rs/$(basename $bin)" "$bin"
    fi
  done

  # meshcore-rs is a library crate, benchmark .rlib file
  if [ -f "../meshcore-rs/target/debug/libmeshcore_rs.rlib" ]; then
    benchmark_binary "meshcore-rs/libmeshcore_rs.rlib" "../meshcore-rs/target/debug/libmeshcore_rs.rlib"
  fi
fi

if [ -d "../meshchat" ]; then
  echo "Building meshchat..."
  (cd ../meshchat && cargo build --quiet 2>/dev/null) || true

  # Find meshchat binaries
  for bin in ../meshchat/target/debug/meshchat*; do
    if [ -f "$bin" ] && [ -x "$bin" ] && [[ ! "$bin" =~ \.d$ ]]; then
      benchmark_binary "meshchat/$(basename $bin)" "$bin"
    fi
  done
fi

if [ -d "../flow" ]; then
  echo "Building flow..."
  (cd ../flow && cargo build --quiet 2>/dev/null) || true

  # Find flow binaries
  for bin in ../flow/target/debug/flow*; do
    if [ -f "$bin" ] && [ -x "$bin" ] && [[ ! "$bin" =~ \.d$ ]]; then
      benchmark_binary "flow/$(basename $bin)" "$bin"
    fi
  done

  # Also check flowc (compiler)
  if [ -f "../flow/target/debug/flowc" ]; then
    benchmark_binary "flow/flowc" "../flow/target/debug/flowc"
  fi

  # Benchmark flow library files
  for rlib in ../flow/target/debug/lib*.rlib; do
    if [ -f "$rlib" ]; then
      benchmark_binary "flow/$(basename $rlib)" "$rlib"
    fi
  done
fi

echo "========================================"  >> "$RESULTS"
echo "Benchmark complete!"  | tee -a "$RESULTS"
echo "Results saved to: $RESULTS"  | tee -a "$RESULTS"
echo "========================================"  >> "$RESULTS"

echo ""
echo "✓ Benchmarks complete!"
echo "Results: $RESULTS"
