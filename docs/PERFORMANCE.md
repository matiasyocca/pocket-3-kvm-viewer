# Performance measurements

## Method

Results in `performance/results.jsonl` are fixed-interval per-process/per-thread Linux `/proc` CPU samples, appended after every completed sample. CPU percentages use **one logical core**; divide by the recorded `logical_cores` for whole-machine capacity. The host has four logical CPUs. Each comparison uses a private D-Bus session, the same 960x540 window, and visible/unfocused, inactive-workspace, and returned states. No FPS or resolution reduction is used in the quality-preserving comparisons.

The original baseline is revision `e399aba`. The `*-confirm` variants have identical opt-in frame-counter instrumentation, so delivered FPS/caps can be checked in both. Instrumentation increments one relaxed atomic counter per buffer and emits a diagnostic every three seconds; normal production builds omit it. These are sink-delivery measurements, not measured monitor presentation FPS or end-to-end latency.

Run the sampler/harness tests:

```sh
python3 -B -m unittest discover -s scripts -p 'test_*.py' -v
```

Build a provisioned release binary and benchmark it (close other viewers first):

```sh
nix develop -c cargo build --release --locked --features benchmark,native-yuv
nix develop -c python3 -B scripts/benchmark.py ./target/release/pocket3-kvm-viewer --label my-variant --output docs/performance/results.jsonl --seconds 15 --repeats 3
```

The harness refuses an existing viewer, targets only a new viewer in its private process group, checks process starttimes and monitor-workspace visibility before/after each sample, bounds compositor calls, and terminates only its private process group. Changed visibility/identity causes an aborted sample, not an accepted record.

## Native YUV comparison (candidate 2)

Three 15-second samples per state; reverse-order confirmation followed the initial baseline/native screening. Medians:

| Variant | Visible CPU, one core | Hidden CPU, one core | Returned CPU, one core |
|---|---:|---:|---:|
| Original baseline, uninstrumented | 56.530% | 36.798% | 54.731% |
| Baseline confirmation, instrumented | 54.797% | 36.931% | 54.263% |
| Native YUV confirmation, instrumented | 21.598% | 1.467% | 21.532% |
| Actual Nix package, native YUV, no instrumentation | 22.399% | 1.467% | 22.465% |

The confirmation reduces visible CPU by 60.586% relative to its paired baseline. The sink receives the same 1920x1080/60 stream: baseline converts YUY2 to RGB; native YUV receives YUY2 directly. Excluding the initial acquisition interval, observed sink-delivery FPS was 59.665–60.332 (baseline) and 59.966–60.316 (native). No capture errors appeared in either confirmation log.

Scoped screenshots of both variants showed the same complete target console with legible green/white text and no obvious rendering artifacts; a changing target build line demonstrated live content. The screenshots remain private in scratch rather than committed. This does not establish full color-chart correctness, touchscreen forwarding, end-to-end latency, GPU energy use, or USB power savings.

RSS did not show a significant regression in these confirmation runs: baseline steady-state observations were 153900–159976 KiB; native 153260–157312 KiB. RSS is not total GPU/device allocation.

Status: accepted after independent review, exact packaged-build verification, three repeated samples per state, and unchanged source mode. The verified output was `/nix/store/kkz4yi9pj1kcli33jhvrazjy4mllwhdw-pocket3-kvm-viewer-0.1.0`. Plain Cargo compatibility remains available; the Nix package explicitly enables native-YUV against GTK >=4.20. `performance/frame-delivery.json` preserves the delivered-FPS/caps evidence. No claim of end-to-end zero-copy is made.

## Transport probes (candidates 5 and 10)

`performance/transport.jsonl` contains bounded raw-source-to-fakesink probes. The first two auto-mode runs completed, then a third stalled. Later modes encountered UVC TRY_FMT EIO/USB probe timeouts; kernel logs also reported UVC DMA allocation failure and xHCI errors. The user subsequently reported successful capture in the viewer, and the native/baseline confirmations above completed normally.

`performance/transport-verdict.json` excludes the affected runs from efficiency comparisons. Their low CPU is a failed/no-frame path, not a performance improvement. No I/O-mode change is accepted from this dataset. Further transport testing must confirm live frames and avoid rapid reopen loops.

## Remaining candidate decisions

Candidate 1 (visibility-driven suspension), 3 (GL), 4 (error cleanup), and the remaining lower-ranked candidates are still being evaluated. No unmeasured fix has been declared a net improvement. Quality-reducing source FPS/resolution changes will not silently replace the default stream; unsupported/ineffective candidates are to be recorded and dropped.

Total energy/power and full end-to-end latency have not been measured. CPU results alone do not prove a total-power reduction.
