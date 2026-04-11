# Instructions for Running Benchmarks on Other Machines

## What to tell Claude on macOS aarch64 or Linux aarch64:

```
I need you to run performance benchmarks for jonesy. Please do the following:

1. Navigate to the jonesy repository:
   cd ~/workspace/jonesy

2. Checkout the benchmark branch and pull the latest changes:
   git fetch origin
   git checkout benchmark_x86_64
   git pull

3. Run the benchmark script:
   ./benchmark.sh

4. Once complete, show me the benchmark results file:
   ls benchmark_results_*.txt
   cat benchmark_results_*.txt
```

## What the script will do:

- Build jonesy in release mode
- Build all test binaries (examples, meshcore-rs, meshchat, flow)
- Run jonesy on each binary 3 times
- Report min/median/max times with millisecond precision
- Count panic detections for verification
- Save results to `benchmark_results_<platform>_<timestamp>.txt`

## Expected output:

You should see output like:
```
Starting benchmarks on Darwin-arm64
Results will be saved to: benchmark_results_Darwin-arm64_20260411_123456.txt

Benchmarking: simple_panic
  Run 1/3... 0.008s
  Run 2/3... 0.007s
  Run 3/3... 0.007s
  Times: min=0.007s, median=0.007s, max=0.008s
  Panic points detected: 33

...
```

## After collecting results from all platforms:

1. Copy the benchmark results files from each machine
2. Compare median times across platforms
3. Calculate performance ratios (x86_64 / aarch64)
4. Identify bottlenecks for optimization

See `benchmark_comparison.md` for analysis guidelines.
